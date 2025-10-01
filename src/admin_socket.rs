use anyhow::Result;
use sqlx::Row;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use crate::{backfill, database::Database};

pub struct AdminSocket {
    db: Arc<Database>,
    socket_path: String,
}

impl AdminSocket {
    pub fn new(db: Arc<Database>, socket_path: String) -> Self {
        Self { db, socket_path }
    }

    pub async fn start(&self) -> Result<()> {
        // Remove old socket if it exists
        let _ = std::fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;
        info!("Admin socket listening on {}", self.socket_path);

        // Set socket permissions so anyone can connect
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&self.socket_path)?.permissions();
            perms.set_mode(0o666);
            std::fs::set_permissions(&self.socket_path, perms)?;
        }

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let db = Arc::clone(&self.db);
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, db).await {
                            error!("Error handling admin connection: {}", e);
                        }
                    });
                }
                Err(e) => {
                    warn!("Failed to accept admin connection: {}", e);
                }
            }
        }
    }
}

async fn handle_connection(stream: UnixStream, db: Arc<Database>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    writer.write_all(b"Feed Generator Admin Console\n").await?;
    writer
        .write_all(b"Commands: backfill <did>, stats, help, quit\n> ")
        .await?;
    writer.flush().await?;

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).await?;

        if bytes_read == 0 {
            break; // Connection closed
        }

        let command = line.trim();
        if command.is_empty() {
            writer.write_all(b"> ").await?;
            writer.flush().await?;
            continue;
        }

        let parts: Vec<&str> = command.split_whitespace().collect();

        match parts.first().copied() {
            Some("backfill") => {
                if let Some(did) = parts.get(1) {
                    writer
                        .write_all(format!("Starting backfill for {}...\n", did).as_bytes())
                        .await?;
                    writer.flush().await?;

                    // First backfill follows
                    match backfill::backfill_follows(Arc::clone(&db), did).await {
                        Ok(_) => {
                            writer
                                .write_all(b"Follows backfilled successfully\n")
                                .await?;
                        }
                        Err(e) => {
                            writer
                                .write_all(format!("Follow backfill failed: {}\n", e).as_bytes())
                                .await?;
                            writer.write_all(b"> ").await?;
                            writer.flush().await?;
                            continue;
                        }
                    }

                    // Then backfill posts
                    writer.write_all(b"Starting post backfill...\n").await?;
                    writer.flush().await?;

                    match backfill::backfill_posts_for_follows(Arc::clone(&db), did, 10).await {
                        Ok(_) => {
                            writer.write_all(b"Posts backfilled successfully\n").await?;
                        }
                        Err(e) => {
                            writer
                                .write_all(format!("Post backfill failed: {}\n", e).as_bytes())
                                .await?;
                        }
                    }
                } else {
                    writer.write_all(b"Usage: backfill <did>\n").await?;
                }
            }
            Some("stats") => match get_stats(&db).await {
                Ok(stats) => {
                    writer.write_all(stats.as_bytes()).await?;
                }
                Err(e) => {
                    writer
                        .write_all(format!("Failed to get stats: {}\n", e).as_bytes())
                        .await?;
                }
            },
            Some("help") => {
                writer.write_all(b"Available commands:\n").await?;
                writer
                    .write_all(b"  backfill <did>  - Backfill follows and posts for a user\n")
                    .await?;
                writer
                    .write_all(b"  stats           - Show database statistics\n")
                    .await?;
                writer
                    .write_all(b"  help            - Show this help message\n")
                    .await?;
                writer
                    .write_all(b"  quit            - Close connection\n")
                    .await?;
            }
            Some("quit") | Some("exit") => {
                writer.write_all(b"Goodbye!\n").await?;
                writer.flush().await?;
                break;
            }
            _ => {
                writer
                    .write_all(
                        format!(
                            "Unknown command: {}. Type 'help' for available commands.\n",
                            command
                        )
                        .as_bytes(),
                    )
                    .await?;
            }
        }

        writer.write_all(b"> ").await?;
        writer.flush().await?;
    }

    Ok(())
}

async fn get_stats(db: &Database) -> Result<String> {
    let post_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM posts")
        .fetch_one(&db.pool)
        .await?
        .try_get("count")?;

    let follow_count: i64 = sqlx::query("SELECT COUNT(*) as count FROM follows")
        .fetch_one(&db.pool)
        .await?
        .try_get("count")?;

    let user_count: i64 = sqlx::query("SELECT COUNT(DISTINCT follower_did) as count FROM follows")
        .fetch_one(&db.pool)
        .await?
        .try_get("count")?;

    Ok(format!(
        "Database Statistics:\n  Posts: {}\n  Follows: {}\n  Users: {}\n",
        post_count, follow_count, user_count
    ))
}
