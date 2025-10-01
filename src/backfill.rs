use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::{
    database::Database,
    types::{Follow, Post},
};

pub async fn backfill_follows(db: Arc<Database>, user_did: &str) -> Result<()> {
    info!("Starting backfill of follows for {}", user_did);

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;
    let mut total_follows = 0;

    loop {
        let mut url = format!(
            "https://public.api.bsky.app/xrpc/app.bsky.graph.getFollows?actor={}&limit=100",
            user_did
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response: serde_json::Value = client.get(&url).send().await?.json().await?;

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
                uri: format!(
                    "at://{}/app.bsky.graph.follow/{}",
                    user_did,
                    uuid::Uuid::new_v4()
                ),
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

pub async fn backfill_posts(db: Arc<Database>, target_did: &str, limit: usize) -> Result<()> {
    debug!("Starting backfill of posts for {}", target_did);

    let client = reqwest::Client::new();
    let mut cursor: Option<String> = None;
    let mut total_posts = 0;
    let mut fetched = 0;

    loop {
        let mut url = format!(
            "https://public.api.bsky.app/xrpc/app.bsky.feed.getAuthorFeed?actor={}&limit=100",
            target_did
        );
        if let Some(ref c) = cursor {
            url.push_str(&format!("&cursor={}", c));
        }

        let response: serde_json::Value = client.get(&url).send().await?.json().await?;

        let feed = response["feed"].as_array();
        if feed.is_none() {
            break;
        }

        for item in feed.unwrap() {
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

            let uri = post["uri"].as_str().unwrap_or("");
            let cid = post["cid"].as_str().unwrap_or("");
            let text = record["text"].as_str().unwrap_or("");
            let created_at_str = record["createdAt"].as_str().unwrap_or("");

            if uri.is_empty() || cid.is_empty() {
                continue;
            }

            let created_at = DateTime::parse_from_rfc3339(created_at_str)
                .unwrap_or_else(|_| Utc::now().into())
                .with_timezone(&Utc);

            let post_record = Post {
                uri: uri.to_string(),
                cid: cid.to_string(),
                author_did: target_did.to_string(),
                text: text.to_string(),
                created_at,
                indexed_at: Utc::now(),
            };

            match db.insert_post(&post_record).await {
                Ok(_) => total_posts += 1,
                Err(e) => debug!("Failed to insert post {}: {}", uri, e),
            }

            fetched += 1;
            if fetched >= limit {
                debug!(
                    "Backfilled {} posts for {} (limit reached)",
                    total_posts, target_did
                );
                return Ok(());
            }
        }

        cursor = response["cursor"].as_str().map(|s| s.to_string());
        if cursor.is_none() {
            break;
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
