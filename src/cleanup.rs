use anyhow::Result;
use futures::StreamExt;
use sqlx::Row;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::database::Database;
use crate::http_client::{create_client, fetch_json_with_retry};

pub async fn verify_active_user_follows(db: Arc<Database>) -> Result<()> {
    info!("Starting follow verification for active users");

    // Only verify follows for users who have accessed the feed in the last 7 days
    let active_users = db.get_active_users(7).await?;
    info!("Verifying follows for {} active users", active_users.len());

    let client = create_client()?;

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

    // Stream follower DIDs to avoid loading all into memory
    let mut follower_stream = sqlx::query("SELECT DISTINCT follower_did FROM follows")
        .fetch(&db.pool);

    // Get active users (accessed feed in last 7 days)
    let active_users = db.get_active_users(7).await?;
    let active_user_set: std::collections::HashSet<String> =
        active_users.into_iter().collect();

    info!("Found {} active users", active_user_set.len());

    // Collect inactive user DIDs in batches for deletion
    let mut inactive_dids: Vec<String> = Vec::new();
    let mut total_users = 0;

    while let Some(row_result) = follower_stream.next().await {
        match row_result {
            Ok(row) => {
                if let Ok(follower_did) = row.try_get::<String, _>("follower_did") {
                    total_users += 1;
                    if !active_user_set.contains(&follower_did) {
                        inactive_dids.push(follower_did);
                    }
                }
            }
            Err(e) => {
                warn!("Error reading follower_did: {}", e);
            }
        }
    }

    info!("Found {} unique users with follows", total_users);

    // Delete follows for inactive users in batches
    let mut deleted_count: u64 = 0;
    for chunk in inactive_dids.chunks(50) {
        for follower_did in chunk {
            let result = sqlx::query("DELETE FROM follows WHERE follower_did = ?")
                .bind(follower_did)
                .execute(&db.pool)
                .await?;

            deleted_count += result.rows_affected();
        }
    }

    if deleted_count > 0 {
        info!("Cleaned up {} follows from inactive users", deleted_count);
    } else {
        debug!("No inactive user follows to clean up");
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

        let response = match fetch_json_with_retry(client, &url).await {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to fetch follows for {}: {}", user_did, e);
                return Err(e);
            }
        };

        let follows = match response["follows"].as_array() {
            Some(f) => f,
            None => {
                if response.get("follows").is_some() {
                    warn!(
                        "Malformed follows response for {}: 'follows' is not an array. Response: {}",
                        user_did,
                        serde_json::to_string(&response).unwrap_or_default()
                    );
                }
                break;
            }
        };

        for follow in follows {
            if let Some(target_did) = follow["did"].as_str() {
                if !target_did.is_empty() {
                    current_follows.push(target_did.to_string());
                }
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
