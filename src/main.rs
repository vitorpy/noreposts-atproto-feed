use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::{StatusCode, HeaderMap},
    response::Json,
    routing::get,
    Router,
};
use clap::Parser;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

mod auth;
mod database;
mod feed_algorithm;
mod jetstream_consumer;
mod publish;
mod types;

use crate::{
    auth::validate_jwt,
    database::Database,
    feed_algorithm::FollowingNoRepostsFeed,
    jetstream_consumer::JetstreamEventHandler,
    types::*,
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

    #[arg(long, env = "JETSTREAM_HOSTNAME", default_value = "jetstream1.us-east.bsky.network")]
    jetstream_hostname: String,
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
    let service_did = args.service_did
        .or_else(|| args.hostname.clone().map(|h| format!("did:web:{}", h)))
        .expect("FEEDGEN_SERVICE_DID or FEEDGEN_HOSTNAME must be set");

    // Initialize database
    let db = Arc::new(Database::new(&args.database_url).await?);
    db.migrate().await?;

    let app_state = AppState {
        db: Arc::clone(&db),
        service_did: service_did.clone(),
    };

    // Start Jetstream consumer
    let event_handler = JetstreamEventHandler::new(Arc::clone(&db));
    tokio::spawn(async move {
        if let Err(e) = event_handler.start(args.jetstream_hostname).await {
            warn!("Jetstream consumer error: {}", e);
        }
    });

    // Setup web server
    let app = Router::new()
        .route("/", get(root))
        .route("/.well-known/did.json", get(did_document))
        .route("/xrpc/app.bsky.feed.getFeedSkeleton", get(get_feed_skeleton))
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
            service_endpoint: format!("https://{}", std::env::var("FEEDGEN_HOSTNAME").unwrap_or_default()),
        }],
    })
}

async fn get_feed_skeleton(
    headers: HeaderMap,
    Query(params): Query<FeedSkeletonParams>,
    State(state): State<AppState>,
) -> Result<Json<FeedSkeletonResponse>, StatusCode> {
    info!("Received feed skeleton request for feed: {}", params.feed);

    // Validate JWT if provided in Authorization header
    let requester_did = if let Some(auth_header) = headers.get("authorization") {
        let auth_str = auth_header.to_str().map_err(|_| {
            warn!("Invalid authorization header format");
            StatusCode::UNAUTHORIZED
        })?;

        // Remove "Bearer " prefix if present
        let token = auth_str.strip_prefix("Bearer ").unwrap_or(auth_str);

        info!("Validating JWT for request");
        match validate_jwt(token, &state.service_did) {
            Ok(claims) => {
                info!("Authenticated request from DID: {}", claims.iss);
                Some(claims.iss)
            },
            Err(e) => {
                warn!("JWT validation failed: {}", e);
                return Err(StatusCode::UNAUTHORIZED);
            }
        }
    } else {
        info!("Unauthenticated request (no Authorization header)");
        None
    };

    let feed_algorithm = FollowingNoRepostsFeed::new(Arc::clone(&state.db));

    info!("Generating feed for requester: {:?}, limit: {:?}, cursor: {:?}",
          requester_did, params.limit, params.cursor);

    match feed_algorithm
        .generate_feed(requester_did, params.limit, params.cursor)
        .await
    {
        Ok(response) => {
            info!("Successfully generated feed with {} posts", response.feed.len());
            Ok(Json(response))
        },
        Err(e) => {
            warn!("Feed generation error: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
