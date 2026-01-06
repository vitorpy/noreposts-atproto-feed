use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::{
    database::Database,
    http_client::{create_client, fetch_json_with_retry},
    types::{Follow, Post},
};

pub async fn backfill_follows(db: Arc<Database>, user_did: &str) -> Result<()> {
    info!("Starting backfill of follows for {}", user_did);

    let client = create_client()?;
    let mut cursor: Option<String> = None;
    let mut total_follows = 0;
    let mut follows_batch: Vec<Follow> = Vec::with_capacity(100);

    loop {
        let mut url = format!(
            "https://public.api.bsky.app/xrpc/app.bsky.graph.getFollows?actor={}&limit=100",
            user_did
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response = match fetch_json_with_retry(&client, &url).await {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to fetch follows for {}: {}", user_did, e);
                break;
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
            let target_did = match follow["did"].as_str() {
                Some(did) if !did.is_empty() => did,
                _ => {
                    debug!("Skipping follow with missing/empty DID: {:?}", follow);
                    continue;
                }
            };

            follows_batch.push(Follow {
                uri: format!(
                    "at://{}/app.bsky.graph.follow/{}",
                    user_did,
                    uuid::Uuid::new_v4()
                ),
                follower_did: user_did.to_string(),
                target_did: target_did.to_string(),
                created_at: chrono::Utc::now(),
                indexed_at: chrono::Utc::now(),
            });
        }

        // Batch insert when we have enough
        if follows_batch.len() >= 100 {
            match db.insert_follows_batch(&follows_batch).await {
                Ok(count) => total_follows += count,
                Err(e) => warn!("Failed to batch insert follows: {}", e),
            }
            follows_batch.clear();
        }

        cursor = response["cursor"].as_str().map(|s| s.to_string());
        if cursor.is_none() {
            break;
        }
    }

    // Insert remaining follows
    if !follows_batch.is_empty() {
        match db.insert_follows_batch(&follows_batch).await {
            Ok(count) => total_follows += count,
            Err(e) => warn!("Failed to batch insert remaining follows: {}", e),
        }
    }

    info!("Backfilled {} follows for {}", total_follows, user_did);
    Ok(())
}

pub async fn backfill_posts(db: Arc<Database>, target_did: &str, limit: usize) -> Result<()> {
    debug!("Starting backfill of posts for {}", target_did);

    let client = create_client()?;
    let mut cursor: Option<String> = None;
    let mut total_posts = 0;
    let mut fetched = 0;
    let mut posts_batch: Vec<Post> = Vec::with_capacity(limit.min(100));

    loop {
        let mut url = format!(
            "https://public.api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed?actor={}&limit=100",
            target_did
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response = match fetch_json_with_retry(&client, &url).await {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to fetch posts for {}: {}", target_did, e);
                break;
            }
        };

        let feed = match response["feed"].as_array() {
            Some(f) => f,
            None => {
                if response.get("feed").is_some() {
                    warn!(
                        "Malformed feed response for {}: 'feed' is not an array. Response: {}",
                        target_did,
                        serde_json::to_string(&response).unwrap_or_default()
                    );
                }
                break;
            }
        };

        for item in feed {
            let post = &item["post"];

            // Skip reposts - check if there's a "reason" field which indicates a repost
            if item.get("reason").is_some() {
                continue;
            }

            // Also check the post record itself for repost indicators
            let record = &post["record"];
            if record.get("subject").is_some() {
                continue; // This is a repost
            }

            let uri = match post["uri"].as_str() {
                Some(u) if !u.is_empty() => u,
                _ => continue,
            };
            let cid = match post["cid"].as_str() {
                Some(c) if !c.is_empty() => c,
                _ => continue,
            };
            let text = record["text"].as_str().unwrap_or("");
            let created_at_str = record["createdAt"].as_str().unwrap_or("");

            let created_at = DateTime::parse_from_rfc3339(created_at_str)
                .unwrap_or_else(|_| Utc::now().into())
                .with_timezone(&Utc);

            posts_batch.push(Post {
                uri: uri.to_string(),
                cid: cid.to_string(),
                author_did: target_did.to_string(),
                text: text.to_string(),
                created_at,
                indexed_at: Utc::now(),
            });

            fetched += 1;
            if fetched >= limit {
                break;
            }
        }

        // Batch insert when we have enough or reached limit
        if posts_batch.len() >= 50 || fetched >= limit {
            match db.insert_posts_batch(&posts_batch).await {
                Ok(count) => total_posts += count,
                Err(e) => debug!("Failed to batch insert posts: {}", e),
            }
            posts_batch.clear();
        }

        if fetched >= limit {
            debug!(
                "Backfilled {} posts for {} (limit reached)",
                total_posts, target_did
            );
            return Ok(());
        }

        cursor = response["cursor"].as_str().map(|s| s.to_string());
        if cursor.is_none() {
            break;
        }
    }

    // Insert remaining posts
    if !posts_batch.is_empty() {
        match db.insert_posts_batch(&posts_batch).await {
            Ok(count) => total_posts += count,
            Err(e) => debug!("Failed to batch insert remaining posts: {}", e),
        }
    }

    debug!("Backfilled {} posts for {}", total_posts, target_did);
    Ok(())
}

pub async fn backfill_posts_for_follows(
    db: Arc<Database>,
    user_did: &str,
    posts_per_user: usize,
) -> Result<()> {
    info!("Starting backfill of posts for {}'s follows", user_did);

    // Get all follows for this user
    let follows = sqlx::query("SELECT target_did FROM follows WHERE follower_did = ?")
        .bind(user_did)
        .fetch_all(&db.pool)
        .await?;

    let total_follows = follows.len();
    info!("Found {} follows to backfill posts from", total_follows);

    for (idx, row) in follows.iter().enumerate() {
        let target_did: String = row.try_get("target_did")?;

        debug!(
            "Backfilling posts from {} ({}/{})",
            target_did,
            idx + 1,
            total_follows
        );

        if let Err(e) = backfill_posts(Arc::clone(&db), &target_did, posts_per_user).await {
            warn!("Failed to backfill posts from {}: {}", target_did, e);
        }

        // Small delay to avoid rate limiting
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    info!("Completed backfill of posts for {}'s follows", user_did);
    Ok(())
}
