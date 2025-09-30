use anyhow::Result;
use atrium_api::{
    agent::atp_agent::{store::MemorySessionStore, AtpAgent},
    app::bsky::feed::generator::RecordData as FeedGeneratorRecord,
    com::atproto::repo::create_record::InputData as CreateRecordInput,
};
use atrium_xrpc_client::reqwest::ReqwestClient;
use serde_json::json;
use std::env;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let identifier = env::var("BLUESKY_IDENTIFIER")?;
    let password = env::var("BLUESKY_PASSWORD")?;
    let hostname = env::var("FEEDGEN_HOSTNAME")?;
    let service_did = env::var("FEEDGEN_SERVICE_DID")?;

    let agent = AtpAgent::new(
        ReqwestClient::new("https://bsky.social"),
        MemorySessionStore::default(),
    );

    // Login to Bluesky
    agent.login(&identifier, &password).await?;

    // Create feed generator record
    let record = FeedGeneratorRecord {
        avatar: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        description: Some("A feed showing posts from people you follow, without any reposts. Clean, chronological timeline of original content only.".to_string()),
        description_facets: None,
        did: service_did.clone(),
        display_name: "Following (No Reposts)".to_string(),
        labels: None,
    };

    let input = CreateRecordInput {
        collection: "app.bsky.feed.generator".parse()?,
        record: record.into(),
        repo: agent.session().await?.did.clone(),
        rkey: Some("following-no-reposts".to_string()),
        swap_commit: None,
        validate: Some(true),
    };

    let response = agent
        .service
        .com
        .atproto
        .repo
        .create_record(input.into())
        .await?;

    println!("Feed generator published successfully!");
    println!("URI: {}", response.uri);
    println!("CID: {}", response.cid);
    println!();
    println!("You can now access your feed at:");
    println!("https://bsky.app/profile/{}/feed/following-no-reposts", agent.session().await?.handle);

    Ok(())
}
