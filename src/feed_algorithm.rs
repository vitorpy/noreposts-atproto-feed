use anyhow::Result;
use std::sync::Arc;
use tracing::warn;

use crate::{
    database::Database,
    types::{FeedSkeletonResponse, SkeletonFeedPost},
};

pub struct FollowingNoRepostsFeed {
    db: Arc<Database>,
}

impl FollowingNoRepostsFeed {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    pub async fn generate_feed(
        &self,
        requester_did: Option<String>,
        limit: Option<i32>,
        cursor: Option<String>,
    ) -> Result<FeedSkeletonResponse> {
        // Require authentication for this feed since it's personalized
        let follower_did = match requester_did {
            Some(did) => did,
            None => {
                warn!("Unauthenticated request to following feed");
                return Ok(FeedSkeletonResponse {
                    cursor: None,
                    feed: vec![],
                });
            }
        };

        let limit = limit.unwrap_or(50).min(100); // Cap at 100 items

        // Get posts from accounts the user follows
        let posts = self
            .db
            .get_following_posts(&follower_did, limit, cursor.as_deref())
            .await?;

        let feed_posts: Vec<SkeletonFeedPost> = posts
            .iter()
            .map(|post| SkeletonFeedPost {
                post: post.uri.clone(),
            })
            .collect();

        // Generate cursor for pagination (use created_at for chronological order)
        let cursor = posts
            .last()
            .map(|post| post.created_at.to_rfc3339());

        Ok(FeedSkeletonResponse {
            cursor,
            feed: feed_posts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Follow, Post};
    use chrono::Utc;

    #[tokio::test]
    async fn test_feed_generation() -> Result<()> {
        let db = Arc::new(Database::new(":memory:").await?);
        db.migrate().await?;

        // Create test data
        let follower_did = "did:example:alice";
        let target_did = "did:example:bob";

        // Insert follow relationship
        let follow = Follow {
            uri: format!("at://{}/app.bsky.graph.follow/test", follower_did),
            follower_did: follower_did.to_string(),
            target_did: target_did.to_string(),
            created_at: Utc::now(),
            indexed_at: Utc::now(),
        };
        db.insert_follow(&follow).await?;

        // Insert post from followed user
        let post = Post {
            uri: format!("at://{}/app.bsky.feed.post/test", target_did),
            cid: "test-cid".to_string(),
            author_did: target_did.to_string(),
            text: "Hello world!".to_string(),
            created_at: Utc::now(),
            indexed_at: Utc::now(),
        };
        db.insert_post(&post).await?;

        let feed_algorithm = FollowingNoRepostsFeed::new(Arc::clone(&db));
        let response = feed_algorithm
            .generate_feed(Some(follower_did.to_string()), Some(10), None)
            .await?;

        assert_eq!(response.feed.len(), 1);
        assert_eq!(response.feed[0].post, post.uri);

        Ok(())
    }
}
