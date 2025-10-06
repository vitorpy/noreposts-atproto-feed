use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

use crate::database::Database;

pub async fn verify_active_user_follows(db: Arc<Database>) -> Result<()> {
    info!("Starting follow verification for active users");

    // Only verify follows for users who have accessed the feed in the last 7 days
    let active_users = db.get_active_users(7).await?;
    info!("Verifying follows for {} active users", active_users.len());

    let client = reqwest::Client::new();

    for user_did in active_users {
        match verify_follows_for_user(&client, Arc::clone(&db), &user_did).await {
            Ok(_) => {
                // Record that we synced this user's follows
                if let Err(e) = db.update_follow_sync(&user_did).await {
                    warn!(
                        "Failed to update follow sync timestamp for {}: {}",
                        user_did, e
                    );
                }
            }
            Err(e) => {
                warn!("Failed to verify follows for {}: {}", user_did, e);
            }
        }
    }

    info!("Follow verification completed");
    Ok(())
}

async fn verify_follows_for_user(
    client: &reqwest::Client,
    db: Arc<Database>,
    user_did: &str,
) -> Result<()> {
    let mut cursor: Option<String> = None;
    let mut current_follows = Vec::new();

    loop {
        let mut url = format!(
            "https://public.api.bsky.app/xrpc/app.bsky.graph.getFollows?actor={}&limit=100",
            user_did
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response: serde_json::Value = match client.get(&url).send().await {
            Ok(r) => r.json().await?,
            Err(e) => {
                warn!("Failed to fetch follows for {}: {}", user_did, e);
                return Err(e.into());
            }
        };

        let follows = response["follows"].as_array();
        if follows.is_none() {
            break;
        }

        for follow in follows.unwrap() {
            if let Some(target_did) = follow["did"].as_str() {
                current_follows.push(target_did.to_string());
            }
        }

        cursor = response["cursor"].as_str().map(|s| s.to_string());
        if cursor.is_none() {
            break;
        }
    }

    // Sync the database with current follows
    db.sync_follows_for_user(user_did, current_follows).await?;

    Ok(())
}
