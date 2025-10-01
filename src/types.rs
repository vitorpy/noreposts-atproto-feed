use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct FeedSkeletonParams {
    pub feed: String,
    pub limit: Option<i32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FeedSkeletonResponse {
    pub cursor: Option<String>,
    pub feed: Vec<SkeletonFeedPost>,
}

#[derive(Debug, Serialize)]
pub struct SkeletonFeedPost {
    pub post: String,
}

#[derive(Debug, Serialize)]
pub struct DidDocument {
    #[serde(rename = "@context")]
    pub context: Vec<String>,
    pub id: String,
    pub service: Vec<ServiceEndpoint>,
}

#[derive(Debug, Serialize)]
pub struct ServiceEndpoint {
    pub id: String,
    #[serde(rename = "type")]
    pub service_type: String,
    #[serde(rename = "serviceEndpoint")]
    pub service_endpoint: String,
}

#[derive(Debug, Clone)]
pub struct Post {
    pub uri: String,
    pub cid: String,
    pub author_did: String,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub indexed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Follow {
    pub uri: String,
    pub follower_did: String,
    pub target_did: String,
    pub created_at: DateTime<Utc>,
    pub indexed_at: DateTime<Utc>,
}

// JWT Claims
#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub iss: String, // issuer (user DID)
    pub aud: String, // audience (feed generator DID)
    pub exp: i64,    // expiration time
}

// ATProto Error Response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}
