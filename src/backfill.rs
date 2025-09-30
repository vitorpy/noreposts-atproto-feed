use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

use crate::{database::Database, types::Follow};

pub async fn backfill_follows(db: Arc<Database>, user_did: &str) -> Result<()> {
    info!("Starting backfill of follows for {}", user_did);

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;
    let mut total_follows = 0;

    loop {
        let mut url = format!("https://public.api.bsky.app/xrpc/app.bsky.graph.getFollows?actor={}&limit=100", user_did);
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response: serde_json::Value = client.get(&url)
            .send()
            .await?
            .json()
            .await?;

        let follows = response["follows"].as_array();
        if follows.is_none() {
            break;
        }

        for follow in follows.unwrap() {
            let target_did = follow["did"].as_str().unwrap_or("");
            if target_did.is_empty() {
                continue;
            }

            let follow_record = Follow {
                uri: format!("at://{}/app.bsky.graph.follow/{}", user_did, uuid::Uuid::new_v4()),
                follower_did: user_did.to_string(),
                target_did: target_did.to_string(),
                created_at: chrono::Utc::now(),
                indexed_at: chrono::Utc::now(),
            };

            match db.insert_follow(&follow_record).await {
                Ok(_) => total_follows += 1,
                Err(e) => warn!("Failed to insert follow {}: {}", target_did, e),
            }
        }

        cursor = response["cursor"].as_str().map(|s| s.to_string());
        if cursor.is_none() {
            break;
        }
    }

    info!("Backfilled {} follows for {}", total_follows, user_did);
    Ok(())
}
