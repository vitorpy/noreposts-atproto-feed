use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::types::{Follow, Post};

pub struct Database {
    pub pool: SqlitePool,
}

impl Database {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = SqlitePool::connect(database_url).await?;

        // Enable WAL mode for better concurrency
        sqlx::query("PRAGMA journal_mode=WAL;")
            .execute(&pool)
            .await?;

        // Set busy timeout to 5 seconds
        sqlx::query("PRAGMA busy_timeout=5000;")
            .execute(&pool)
            .await?;

        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    // Post operations
    pub async fn insert_post(&self, post: &Post) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO posts (uri, cid, author_did, text, created_at, indexed_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&post.uri)
        .bind(&post.cid)
        .bind(&post.author_did)
        .bind(&post.text)
        .bind(post.created_at.to_rfc3339())
        .bind(post.indexed_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_post(&self, uri: &str) -> Result<()> {
        sqlx::query("DELETE FROM posts WHERE uri = ?")
            .bind(uri)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // Follow operations
    pub async fn insert_follow(&self, follow: &Follow) -> Result<()> {
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO follows (uri, follower_did, target_did, created_at, indexed_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(&follow.uri)
        .bind(&follow.follower_did)
        .bind(&follow.target_did)
        .bind(follow.created_at.to_rfc3339())
        .bind(follow.indexed_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_follow(&self, uri: &str) -> Result<()> {
        sqlx::query("DELETE FROM follows WHERE uri = ?")
            .bind(uri)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // Feed generation queries
    pub async fn get_following_posts(
        &self,
        follower_did: &str,
        limit: i32,
        cursor: Option<&str>,
    ) -> Result<Vec<Post>> {
        let cursor_time = cursor
            .and_then(|c| DateTime::parse_from_rfc3339(c).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        let rows = sqlx::query(
            r#"
            SELECT p.uri, p.cid, p.author_did, p.text, p.created_at, p.indexed_at
            FROM posts p
            INNER JOIN follows f ON f.target_did = p.author_did
            WHERE f.follower_did = ?
                AND p.created_at < ?
            ORDER BY p.created_at DESC
            LIMIT ?
            "#,
        )
        .bind(follower_did)
        .bind(cursor_time.to_rfc3339())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        let mut posts = Vec::new();
        for row in rows {
            let uri: String = row.try_get("uri")?;
            let cid: String = row.try_get("cid")?;
            let author_did: String = row.try_get("author_did")?;
            let text: String = row.try_get("text")?;
            let created_at_str: String = row.try_get("created_at")?;
            let indexed_at_str: String = row.try_get("indexed_at")?;

            posts.push(Post {
                uri,
                cid,
                author_did,
                text,
                created_at: DateTime::parse_from_rfc3339(&created_at_str)?.with_timezone(&Utc),
                indexed_at: DateTime::parse_from_rfc3339(&indexed_at_str)?.with_timezone(&Utc),
            });
        }

        Ok(posts)
    }

    pub async fn cleanup_old_posts(&self, hours: i64) -> Result<()> {
        let cutoff = Utc::now() - chrono::Duration::hours(hours);
        let result = sqlx::query("DELETE FROM posts WHERE indexed_at < ?")
            .bind(cutoff.to_rfc3339())
            .execute(&self.pool)
            .await?;

        let deleted = result.rows_affected();
        if deleted > 0 {
            tracing::info!("Cleaned up {} posts older than {} hours", deleted, hours);
        }
        Ok(())
    }

    pub async fn record_feed_request(&self, user_did: &str) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO active_users (did, last_feed_request)
            VALUES (?, ?)
            ON CONFLICT(did) DO UPDATE SET last_feed_request = excluded.last_feed_request
            "#,
        )
        .bind(user_did)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_active_users(&self, days: i64) -> Result<Vec<String>> {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        let rows = sqlx::query(
            "SELECT did FROM active_users WHERE last_feed_request > ? ORDER BY last_feed_request DESC"
        )
        .bind(cutoff.to_rfc3339())
        .fetch_all(&self.pool)
        .await?;

        let dids: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.try_get("did").ok())
            .collect();

        Ok(dids)
    }

    pub async fn update_follow_sync(&self, user_did: &str) -> Result<()> {
        sqlx::query("UPDATE active_users SET last_follow_sync = ? WHERE did = ?")
            .bind(Utc::now().to_rfc3339())
            .bind(user_did)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn sync_follows_for_user(
        &self,
        user_did: &str,
        current_target_dids: Vec<String>,
    ) -> Result<()> {
        // Get all follows for this user in our database
        let rows = sqlx::query("SELECT target_did FROM follows WHERE follower_did = ?")
            .bind(user_did)
            .fetch_all(&self.pool)
            .await?;

        let db_target_dids: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.try_get("target_did").ok())
            .collect();

        // Find follows in database that no longer exist in current follows
        let mut removed_count = 0;
        for db_target in &db_target_dids {
            if !current_target_dids.contains(db_target) {
                // This follow no longer exists, remove it
                sqlx::query("DELETE FROM follows WHERE follower_did = ? AND target_did = ?")
                    .bind(user_did)
                    .bind(db_target)
                    .execute(&self.pool)
                    .await?;
                removed_count += 1;
                tracing::info!("Removed stale follow: {} -> {}", user_did, db_target);
            }
        }

        if removed_count > 0 {
            tracing::info!(
                "Cleaned up {} stale follows for {}",
                removed_count,
                user_did
            );
        }

        Ok(())
    }

    // Unused but kept for potential future use
    #[allow(dead_code)]
    pub async fn is_following(&self, follower_did: &str, target_did: &str) -> Result<bool> {
        let row = sqlx::query(
            "SELECT COUNT(*) as count FROM follows WHERE follower_did = ? AND target_did = ?",
        )
        .bind(follower_did)
        .bind(target_did)
        .fetch_one(&self.pool)
        .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count > 0)
    }
}
