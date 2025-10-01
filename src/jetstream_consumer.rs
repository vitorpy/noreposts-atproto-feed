use anyhow::Result;
use atproto_jetstream::{Consumer, ConsumerTaskConfig, EventHandler, JetstreamEvent, CancellationToken};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tracing::{error, warn, info, debug};

use crate::{database::Database, types::{Follow, Post}};

pub struct JetstreamEventHandler {
    db: Arc<Database>,
}

impl JetstreamEventHandler {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn start(&self, jetstream_hostname: String) -> Result<()> {
        info!("Starting Jetstream consumer, connecting to {}", jetstream_hostname);

        let config = ConsumerTaskConfig {
            user_agent: "following-no-reposts-feed/1.0".to_string(),
            compression: true,
            jetstream_hostname: jetstream_hostname.clone(),
            zstd_dictionary_location: String::new(),
            collections: vec![
                "app.bsky.feed.post".to_string(),
                "app.bsky.graph.follow".to_string(),
            ],
            dids: vec![],
            cursor: None,
            max_message_size_bytes: None,
            require_hello: false,
        };

        info!("Jetstream config: compression={}, collections={:?}, require_hello={}",
              config.compression, config.collections, config.require_hello);

        let consumer = Consumer::new(config);
        info!("Consumer created, registering event handler...");
        consumer.register_handler(Arc::new(self.clone())).await?;
        info!("Event handler registered successfully");

        let cancellation_token = CancellationToken::new();
        info!("Starting Jetstream consumer background task...");

        // Start cleanup task
        let db_cleanup = Arc::clone(&self.db);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Every hour
            loop {
                interval.tick().await;
                if let Err(e) = db_cleanup.cleanup_old_posts(48).await {
                    warn!("Failed to cleanup old posts: {}", e);
                }
            }
        });

        info!("Calling consumer.run_background()...");
        consumer.run_background(cancellation_token).await?;
        info!("Consumer.run_background() returned");
        Ok(())
    }
}

impl Clone for JetstreamEventHandler {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

#[async_trait]
impl EventHandler for JetstreamEventHandler {
    async fn handle_event(&self, event: JetstreamEvent) -> anyhow::Result<()> {
        match event {
            JetstreamEvent::Commit { did, time_us: _, kind, commit } => {
                info!("EVENT RECEIVED - Commit: did={}, kind={:?}, collection={}, operation={}",
                      did, kind, commit.collection, commit.operation);

                match commit.collection.as_str() {
                    "app.bsky.feed.post" => {
                        info!("Processing post event from {}", did);
                        self.handle_post_event(&did, &commit.collection, &commit.rkey, &commit.operation, Some(&commit.record), &commit.cid).await?;
                    }
                    "app.bsky.graph.follow" => {
                        info!("Processing follow event: {} -> target", did);
                        self.handle_follow_event(&did, &commit.collection, &commit.rkey, &commit.operation, Some(&commit.record)).await?;
                    }
                    _ => {
                        info!("Ignoring collection: {}", commit.collection);
                    }
                }
            }
            JetstreamEvent::Account { did, kind, account, .. } => {
                info!("EVENT RECEIVED - Account: did={}, kind={:?}", did, kind);
            }
            JetstreamEvent::Identity { did, kind, identity, .. } => {
                info!("EVENT RECEIVED - Identity: did={}, kind={:?}", did, kind);
            }
            JetstreamEvent::Delete { did, kind, .. } => {
                info!("EVENT RECEIVED - Delete: did={}, kind={:?}", did, kind);
            }
        }
        Ok(())
    }

    fn handler_id(&self) -> String {
        "following-no-reposts-handler".to_string()
    }
}

impl JetstreamEventHandler {
    async fn handle_post_event(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
        operation: &str,
        record: Option<&serde_json::Value>,
        cid: &str,
    ) -> Result<()> {
        let uri = format!("at://{}/{}/{}", did, collection, rkey);

        match operation {
            "create" => {
                if let Some(record) = record {
                    // Check if this is a repost by looking for a "subject" field
                    if record.get("subject").is_some() {
                        // This is a repost, skip it
                        return Ok(());
                    }

                    let text = record
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let created_at_str = record
                        .get("createdAt")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let created_at = DateTime::parse_from_rfc3339(created_at_str)
                        .unwrap_or_else(|_| Utc::now().into())
                        .with_timezone(&Utc);

                    let post = Post {
                        uri: uri.clone(),
                        cid: cid.to_string(),
                        author_did: did.to_string(),
                        text: text.clone(),
                        created_at,
                        indexed_at: Utc::now(),
                    };

                    if let Err(e) = self.db.insert_post(&post).await {
                        error!("Failed to insert post: {}", e);
                    } else {
                        debug!("Inserted post: {} by {}", uri, did);
                    }
                }
            }
            "delete" => {
                if let Err(e) = self.db.delete_post(&uri).await {
                    error!("Failed to delete post: {}", e);
                }
            }
            _ => {} // Ignore updates
        }

        Ok(())
    }

    async fn handle_follow_event(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
        operation: &str,
        record: Option<&serde_json::Value>,
    ) -> Result<()> {
        let uri = format!("at://{}/{}/{}", did, collection, rkey);

        match operation {
            "create" => {
                if let Some(record) = record {
                    let target_did = record
                        .get("subject")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let created_at_str = record
                        .get("createdAt")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    let created_at = DateTime::parse_from_rfc3339(created_at_str)
                        .unwrap_or_else(|_| Utc::now().into())
                        .with_timezone(&Utc);

                    let follow = Follow {
                        uri: uri.clone(),
                        follower_did: did.to_string(),
                        target_did: target_did.clone(),
                        created_at,
                        indexed_at: Utc::now(),
                    };

                    if let Err(e) = self.db.insert_follow(&follow).await {
                        error!("Failed to insert follow: {}", e);
                    } else {
                        info!("Inserted follow: {} -> {}", did, target_did);
                    }
                }
            }
            "delete" => {
                if let Err(e) = self.db.delete_follow(&uri).await {
                    error!("Failed to delete follow: {}", e);
                }
            }
            _ => {} // Ignore updates
        }

        Ok(())
    }
}
