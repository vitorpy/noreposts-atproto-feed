use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use clap::Parser;
use sqlx::Row;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

mod admin_socket;
mod auth;
mod backfill;
mod database;
mod feed_algorithm;
mod jetstream_consumer;
mod publish;
mod types;

use crate::{
    admin_socket::AdminSocket, auth::validate_jwt, database::Database,
    feed_algorithm::FollowingNoRepostsFeed, jetstream_consumer::JetstreamEventHandler, types::*,
};

#[derive(Parser)]
#[command(name = "following-no-reposts-feed")]
#[command(about = "A Bluesky feed generator for following without reposts")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long, env = "DATABASE_URL", default_value = "sqlite:./feed.db")]
    database_url: String,

    #[arg(long, env = "PORT", default_value = "3000")]
    port: u16,

    #[arg(long, env = "FEEDGEN_HOSTNAME")]
    hostname: Option<String>,

    #[arg(long, env = "FEEDGEN_SERVICE_DID")]
    service_did: Option<String>,

    #[arg(
        long,
        env = "JETSTREAM_HOSTNAME",
        default_value = "jetstream1.us-east.bsky.network"
    )]
    jetstream_hostname: String,

    #[arg(
        long,
        env = "ADMIN_SOCKET",
        default_value = "/var/run/noreposts-feed.sock"
    )]
    admin_socket: String,
}

#[derive(Parser)]
enum Command {
    /// Publish the feed to Bluesky
    Publish,
    /// Run the feed generator server (default)
    Serve,
}

#[derive(Clone)]
struct AppState {
    db: Arc<Database>,
    service_did: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    let args = Args::parse();

    // Handle publish command
    if matches!(args.command, Some(Command::Publish)) {
        return publish::publish_feed().await;
    }

    // Default to serve mode
    let service_did = args
        .service_did
        .or_else(|| args.hostname.clone().map(|h| format!("did:web:{}", h)))
        .expect("FEEDGEN_SERVICE_DID or FEEDGEN_HOSTNAME must be set");

    // Initialize database
    let db = Arc::new(Database::new(&args.database_url).await?);
    db.migrate().await?;

    let app_state = AppState {
        db: Arc::clone(&db),
        service_did: service_did.clone(),
    };

    // Start admin socket
    let admin_socket = AdminSocket::new(Arc::clone(&db), args.admin_socket.clone());
    tokio::spawn(async move {
        if let Err(e) = admin_socket.start().await {
            warn!("Admin socket error: {}", e);
        }
    });

    // Start cleanup task
    let db_cleanup = Arc::clone(&db);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Every hour
        loop {
            interval.tick().await;
            if let Err(e) = db_cleanup.cleanup_old_posts(48).await {
                warn!("Failed to cleanup old posts: {}", e);
            }
        }
    });

    // Start Jetstream consumer with automatic reconnection
    let event_handler = JetstreamEventHandler::new(Arc::clone(&db));
    let jetstream_hostname = args.jetstream_hostname.clone();
    tokio::spawn(async move {
        loop {
            info!("Starting Jetstream consumer...");
            if let Err(e) = event_handler.start(jetstream_hostname.clone()).await {
                warn!(
                    "Jetstream consumer error: {}. Reconnecting in 5 seconds...",
                    e
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            } else {
                // Consumer stopped without error, wait before restarting
                warn!("Jetstream consumer stopped unexpectedly. Reconnecting in 5 seconds...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }
    });

    // Setup web server
    let app = Router::new()
        .route("/", get(root))
        .route("/.well-known/did.json", get(did_document))
        .route(
            "/xrpc/app.bsky.feed.getFeedSkeleton",
            get(get_feed_skeleton),
        )
        .layer(CorsLayer::permissive())
        .with_state(app_state);

    let listener = TcpListener::bind(format!("0.0.0.0:{}", args.port)).await?;
    info!("Feed generator listening on port {}", args.port);

    axum::serve(listener, app).await?;
    Ok(())
}

async fn root() -> &'static str {
    "Following No Reposts Feed Generator"
}

async fn did_document(State(state): State<AppState>) -> Json<DidDocument> {
    Json(DidDocument {
        context: vec!["https://www.w3.org/ns/did/v1".to_string()],
        id: state.service_did.clone(),
        service: vec![ServiceEndpoint {
            id: "#bsky_fg".to_string(),
            service_type: "BskyFeedGenerator".to_string(),
            service_endpoint: format!(
                "https://{}",
                std::env::var("FEEDGEN_HOSTNAME").unwrap_or_default()
            ),
        }],
    })
}

async fn get_feed_skeleton(
    headers: HeaderMap,
    Query(params): Query<FeedSkeletonParams>,
    State(state): State<AppState>,
) -> Response {
    info!("Received feed skeleton request for feed: {}", params.feed);

    // This feed requires authentication since it's personalized
    let auth_header = match headers.get("authorization") {
        Some(h) => h,
        None => {
            warn!("Missing Authorization header - this feed requires authentication");
            return (
                StatusCode::UNAUTHORIZED,
                Json(types::ErrorResponse {
                    error: "AuthenticationRequired".to_string(),
                    message:
                        "This feed shows posts from accounts you follow and requires authentication"
                            .to_string(),
                }),
            )
                .into_response();
        }
    };

    let auth_str = match auth_header.to_str() {
        Ok(s) => s,
        Err(_) => {
            warn!("Invalid authorization header format");
            return (
                StatusCode::UNAUTHORIZED,
                Json(types::ErrorResponse {
                    error: "AuthenticationRequired".to_string(),
                    message: "Invalid authorization header format".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Remove "Bearer " prefix if present
    let token = auth_str.strip_prefix("Bearer ").unwrap_or(auth_str);

    info!("Validating JWT for request");
    let requester_did = match validate_jwt(token, &state.service_did) {
        Ok(claims) => {
            info!("Authenticated request from DID: {}", claims.iss);
            claims.iss
        }
        Err(e) => {
            warn!("JWT validation failed: {}", e);
            return (
                StatusCode::UNAUTHORIZED,
                Json(types::ErrorResponse {
                    error: "AuthenticationRequired".to_string(),
                    message: format!("JWT validation failed: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Check if user has any follows, if not, backfill them and their posts
    let db_for_backfill = Arc::clone(&state.db);
    let requester_did_clone = requester_did.clone();
    tokio::spawn(async move {
        // Check if we have any follows for this user
        let has_follows =
            sqlx::query("SELECT COUNT(*) as count FROM follows WHERE follower_did = ?")
                .bind(&requester_did_clone)
                .fetch_one(&db_for_backfill.pool)
                .await
                .ok()
                .and_then(|row| row.try_get::<i64, _>("count").ok())
                .unwrap_or(0);

        if has_follows == 0 {
            info!(
                "No follows found for {}, triggering backfill",
                requester_did_clone
            );

            // First backfill follows
            if let Err(e) =
                backfill::backfill_follows(Arc::clone(&db_for_backfill), &requester_did_clone).await
            {
                warn!("Follow backfill failed for {}: {}", requester_did_clone, e);
                return;
            }

            // Then backfill recent posts from each follow (10 posts per user)
            info!("Starting post backfill for {}", requester_did_clone);
            if let Err(e) = backfill::backfill_posts_for_follows(
                Arc::clone(&db_for_backfill),
                &requester_did_clone,
                10,
            )
            .await
            {
                warn!("Post backfill failed for {}: {}", requester_did_clone, e);
            }
        }
    });

    let feed_algorithm = FollowingNoRepostsFeed::new(Arc::clone(&state.db));

    info!(
        "Generating feed for requester: {}, limit: {:?}, cursor: {:?}",
        requester_did, params.limit, params.cursor
    );

    match feed_algorithm
        .generate_feed(Some(requester_did), params.limit, params.cursor)
        .await
    {
        Ok(response) => {
            info!(
                "Successfully generated feed with {} posts",
                response.feed.len()
            );
            Json(response).into_response()
        }
        Err(e) => {
            warn!("Feed generation error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(types::ErrorResponse {
                    error: "InternalServerError".to_string(),
                    message: format!("Failed to generate feed: {}", e),
                }),
            )
                .into_response()
        }
    }
}
