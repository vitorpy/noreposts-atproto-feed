use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

use crate::database::Database;

pub async fn verify_all_follows(db: Arc<Database>) -> Result<()> {
    info!("Starting follow verification cleanup");

    let follower_dids = db.get_all_follower_dids().await?;
    info!("Verifying follows for {} users", follower_dids.len());

    let client = reqwest::Client::new();

    for follower_did in follower_dids {
        match verify_follows_for_user(&client, Arc::clone(&db), &follower_did).await {
            Ok(_) => {}
            Err(e) => {
                warn!("Failed to verify follows for {}: {}", follower_did, e);
            }
        }
    }

    info!("Follow verification cleanup completed");
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
