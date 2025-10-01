use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};
use url::Url;

use crate::{database::Database, types::{Follow, Post}};

pub struct JetstreamEventHandler {
    db: Arc<Database>,
}

impl JetstreamEventHandler {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn start(&self, jetstream_hostname: String) -> Result<()> {
        let wanted_collections = "wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.graph.follow";
        let ws_url = format!("wss://{}/subscribe?{}", jetstream_hostname, wanted_collections);

        info!("Connecting to Jetstream at {}", ws_url);

        loop {
            match tokio_tungstenite::connect_async(Url::parse(&ws_url)?).await {
                Ok((mut socket, _response)) => {
                    info!("Connected to Jetstream successfully");

                    while let Some(msg) = socket.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                if let Err(e) = self.handle_message(&text).await {
                                    error!("Error handling message: {}", e);
                                }
                            }
                            Ok(Message::Close(_)) => {
                                warn!("Jetstream connection closed");
                                break;
                            }
                            Err(e) => {
                                error!("WebSocket error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Jetstream: {}. Reconnecting in 5 seconds...", e);
                }
            }

            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    async fn handle_message(&self, message: &str) -> Result<()> {
        let event: JetstreamEvent = serde_json::from_str(message)?;

        match event {
            JetstreamEvent::Commit { did, commit, .. } => {
                info!("Received commit event: did={}, collection={}, operation={}",
                      did, commit.collection, commit.operation);

                match commit.collection.as_str() {
                    "app.bsky.feed.post" => {
                        self.handle_post_event(&did, &commit).await?;
                    }
                    "app.bsky.graph.follow" => {
                        self.handle_follow_event(&did, &commit).await?;
                    }
                    _ => {}
                }
            }
            JetstreamEvent::Account { did, .. } => {
                info!("Received account event: did={}", did);
            }
            JetstreamEvent::Identity { did, .. } => {
                info!("Received identity event: did={}", did);
            }
        }

        Ok(())
    }

    async fn handle_post_event(&self, did: &str, commit: &JetstreamCommit) -> Result<()> {
        let uri = format!("at://{}/{}/{}", did, commit.collection, commit.rkey);

        match commit.operation.as_str() {
            "create" => {
                if let Some(record) = &commit.record {
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

                    let cid = commit.cid.as_ref().unwrap_or(&String::new()).clone();

                    let post = Post {
                        uri: uri.clone(),
                        cid,
                        author_did: did.to_string(),
                        text: text.clone(),
                        created_at,
                        indexed_at: Utc::now(),
                    };

                    if let Err(e) = self.db.insert_post(&post).await {
                        error!("Failed to insert post: {}", e);
                    } else {
                        info!("Inserted post: {} by {}", uri, did);
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

    async fn handle_follow_event(&self, did: &str, commit: &JetstreamCommit) -> Result<()> {
        let uri = format!("at://{}/{}/{}", did, commit.collection, commit.rkey);

        match commit.operation.as_str() {
            "create" => {
                if let Some(record) = &commit.record {
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

impl Clone for JetstreamEventHandler {
    fn clone(&self) -> Self {
        Self {
            db: Arc::clone(&self.db),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind")]
enum JetstreamEvent {
    #[serde(rename = "commit")]
    Commit {
        did: String,
        time_us: i64,
        commit: JetstreamCommit,
    },
    #[serde(rename = "account")]
    Account {
        did: String,
        time_us: i64,
        account: serde_json::Value,
    },
    #[serde(rename = "identity")]
    Identity {
        did: String,
        time_us: i64,
        identity: serde_json::Value,
    },
}

#[derive(Debug, Deserialize, Serialize)]
struct JetstreamCommit {
    rev: String,
    operation: String,
    collection: String,
    rkey: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cid: Option<String>,
}
