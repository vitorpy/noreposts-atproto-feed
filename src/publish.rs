use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};

#[derive(Debug, Serialize)]
struct LoginRequest {
    identifier: String,
    password: String,
}

#[derive(Debug, Deserialize)]
struct LoginResponse {
    #[serde(rename = "accessJwt")]
    access_jwt: String,
    did: String,
    handle: String,
}

#[derive(Debug, Serialize)]
struct PutRecordRequest {
    repo: String,
    collection: String,
    rkey: String,
    record: FeedGeneratorRecord,
}

#[derive(Debug, Serialize)]
struct FeedGeneratorRecord {
    #[serde(rename = "$type")]
    record_type: String,
    did: String,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

pub async fn publish_feed() -> Result<()> {
    println!("=== Bluesky Feed Generator Publisher ===\n");

    // Get user input
    let handle = prompt("Enter your Bluesky handle: ")?;
    let password = prompt_password("Enter your Bluesky password (App Password): ")?;
    let record_name = prompt("Enter a short name for the record (shown in URL): ")?;
    let display_name = prompt("Enter a display name for your feed: ")?;
    let description = prompt_optional("Enter a brief description (optional): ")?;

    // Get feed generator DID from environment
    dotenvy::dotenv().ok();
    let feedgen_service_did = std::env::var("FEEDGEN_SERVICE_DID")
        .or_else(|_| {
            std::env::var("FEEDGEN_HOSTNAME").map(|hostname| format!("did:web:{}", hostname))
        })
        .map_err(|_| anyhow!("Please set FEEDGEN_SERVICE_DID or FEEDGEN_HOSTNAME in .env file"))?;

    println!("\nPublishing feed...");

    let client = Client::new();
    let pds_url = "https://bsky.social";

    // Login to get session
    let login_response: LoginResponse = client
        .post(format!("{}/xrpc/com.atproto.server.createSession", pds_url))
        .json(&LoginRequest {
            identifier: handle.clone(),
            password,
        })
        .send()
        .await?
        .json()
        .await?;

    println!("✓ Logged in as {}", login_response.did);

    // Create feed generator record
    let record = FeedGeneratorRecord {
        record_type: "app.bsky.feed.generator".to_string(),
        did: feedgen_service_did,
        display_name,
        description: if description.is_empty() {
            None
        } else {
            Some(description)
        },
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    // Publish the record
    let put_request = PutRecordRequest {
        repo: login_response.did.clone(),
        collection: "app.bsky.feed.generator".to_string(),
        rkey: record_name.clone(),
        record,
    };

    let response = client
        .post(format!("{}/xrpc/com.atproto.repo.putRecord", pds_url))
        .header(
            "Authorization",
            format!("Bearer {}", login_response.access_jwt),
        )
        .json(&put_request)
        .send()
        .await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        eprintln!("Error response: {}", error_text);
        return Err(anyhow!("Failed to publish feed: {}", error_text));
    }

    response.error_for_status()?;

    println!("\n✅ Feed published successfully!");
    println!(
        "🔗 Feed AT-URI: at://{}/app.bsky.feed.generator/{}",
        login_response.did, record_name
    );
    println!("\n🌐 You can view your feed at:");
    println!(
        "   https://bsky.app/profile/{}/feed/{}",
        login_response.handle, record_name
    );
    println!("\nYou can now find and share your feed in the Bluesky app!");

    Ok(())
}

fn prompt(message: &str) -> Result<String> {
    print!("{}", message);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_optional(message: &str) -> Result<String> {
    print!("{}", message);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_password(message: &str) -> Result<String> {
    print!("{}", message);
    io::stdout().flush()?;
    let mut password = String::new();
    io::stdin().read_line(&mut password)?;
    Ok(password.trim().to_string())
}
