use anyhow::Result;
use sqlx::Row;
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

pub async fn cleanup_inactive_user_follows(db: Arc<Database>) -> Result<()> {
    info!("Starting cleanup of follows for inactive users");

    // Get all unique follower DIDs from the follows table
    let all_follower_dids: Vec<String> = sqlx::query("SELECT DISTINCT follower_did FROM follows")
        .fetch_all(&db.pool)
        .await?
        .into_iter()
        .filter_map(|row| row.try_get("follower_did").ok())
        .collect();

    info!("Found {} unique users with follows", all_follower_dids.len());

    // Get active users (accessed feed in last 7 days)
    let active_users = db.get_active_users(7).await?;
    let active_user_set: std::collections::HashSet<String> =
        active_users.into_iter().collect();

    // Delete follows for users who are not active
    let mut deleted_count = 0;
    for follower_did in all_follower_dids {
        if !active_user_set.contains(&follower_did) {
            let result = sqlx::query("DELETE FROM follows WHERE follower_did = ?")
                .bind(&follower_did)
                .execute(&db.pool)
                .await?;

            deleted_count += result.rows_affected();
        }
    }

    if deleted_count > 0 {
        info!("Cleaned up {} follows from inactive users", deleted_count);
    } else {
        info!("No inactive user follows to clean up");
    }

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
