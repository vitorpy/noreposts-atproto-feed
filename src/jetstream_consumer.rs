use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

use crate::{
    database::Database,
    types::{Follow, Post},
};

/// Maximum backoff delay in seconds
const MAX_BACKOFF_SECS: u64 = 60;
/// Initial backoff delay in seconds
const INITIAL_BACKOFF_SECS: u64 = 1;
/// Timeout for receiving messages (detects dead connections)
const MESSAGE_TIMEOUT_SECS: u64 = 90;

pub struct JetstreamEventHandler {
    db: Arc<Database>,
    consecutive_failures: AtomicU64,
}

impl JetstreamEventHandler {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            consecutive_failures: AtomicU64::new(0),
        }
    }

    /// Check if Jetstream is connected (for health checks)
    pub fn is_healthy(&self) -> bool {
        // Consider unhealthy after 3 consecutive failures
        self.consecutive_failures.load(Ordering::Relaxed) < 3
    }

    fn get_backoff_duration(&self) -> Duration {
        let failures = self.consecutive_failures.load(Ordering::Relaxed);
        let backoff_secs = INITIAL_BACKOFF_SECS.saturating_mul(2u64.saturating_pow(failures as u32));
        Duration::from_secs(backoff_secs.min(MAX_BACKOFF_SECS))
    }

    fn reset_backoff(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    fn increment_backoff(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    pub async fn start(&self, jetstream_hostname: String) -> Result<()> {
        let wanted_collections =
            "wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.graph.follow";
        let ws_url = format!(
            "wss://{}/subscribe?{}",
            jetstream_hostname, wanted_collections
        );

        info!("Connecting to Jetstream at {}", ws_url);

        loop {
            match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((mut socket, _response)) => {
                    info!("Connected to Jetstream successfully");
                    self.reset_backoff();

                    loop {
                        // Use timeout to detect dead connections
                        let msg_result = tokio::time::timeout(
                            Duration::from_secs(MESSAGE_TIMEOUT_SECS),
                            socket.next(),
                        )
                        .await;

                        match msg_result {
                            Ok(Some(Ok(Message::Text(text)))) => {
                                if let Err(e) = self.handle_message(&text).await {
                                    debug!("Error handling message: {}", e);
                                }
                            }
                            Ok(Some(Ok(Message::Ping(data)))) => {
                                // Respond to pings to keep connection alive
                                if let Err(e) = futures::SinkExt::send(
                                    &mut socket,
                                    Message::Pong(data),
                                )
                                .await
                                {
                                    warn!("Failed to send pong: {}", e);
                                    break;
                                }
                            }
                            Ok(Some(Ok(Message::Close(_)))) => {
                                warn!("Jetstream connection closed by server");
                                self.increment_backoff();
                                break;
                            }
                            Ok(Some(Err(e))) => {
                                error!("WebSocket error: {}", e);
                                self.increment_backoff();
                                break;
                            }
                            Ok(None) => {
                                warn!("Jetstream connection ended");
                                self.increment_backoff();
                                break;
                            }
                            Err(_) => {
                                warn!(
                                    "No message received in {} seconds, reconnecting...",
                                    MESSAGE_TIMEOUT_SECS
                                );
                                self.increment_backoff();
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to connect to Jetstream: {}", e);
                    self.increment_backoff();
                }
            }

            let backoff = self.get_backoff_duration();
            info!("Reconnecting to Jetstream in {:?}...", backoff);
            tokio::time::sleep(backoff).await;
        }
    }

    async fn handle_message(&self, message: &str) -> Result<()> {
        let event: JetstreamEvent = serde_json::from_str(message)?;

        match event {
            JetstreamEvent::Commit { did, commit, .. } => {
                debug!(
                    "Received commit event: did={}, collection={}, operation={}",
                    did, commit.collection, commit.operation
                );

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
                debug!("Received account event: did={}", did);
            }
            JetstreamEvent::Identity { did, .. } => {
                debug!("Received identity event: did={}", did);
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
                        debug!("Inserted post: {} by {}", uri, did);
                    }
                }
            }
            "delete" => {
                if let Err(e) = self.db.delete_post(&uri).await {
                    error!("Failed to delete post: {}", e);
                } else {
                    debug!("Deleted post: {}", uri);
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
                        debug!("Inserted follow: {} -> {}", did, target_did);
                    }
                }
            }
            "delete" => {
                if let Err(e) = self.db.delete_follow(&uri).await {
                    error!("Failed to delete follow: {}", e);
                } else {
                    debug!("Deleted follow: {}", uri);
                }
            }
            _ => {} // Ignore updates
        }

        Ok(())
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
