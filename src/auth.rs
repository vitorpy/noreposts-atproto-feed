use anyhow::{anyhow, Result};
use atrium_common::resolver::Resolver;
use atrium_crypto::did::{format_did_key, parse_multikey};
use atrium_identity::did::{CommonDidResolver, CommonDidResolverConfig, DEFAULT_PLC_DIRECTORY_URL};
use atrium_xrpc_client::reqwest::ReqwestClient;
use base64::Engine;
use jwt_compact::UntrustedToken;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::types::JwtClaims;

// Unused structs kept for reference if needed in future
// #[derive(Debug, Deserialize)]
// struct EmptyCustomClaims {}
//
// #[derive(Debug, Deserialize)]
// struct StandardClaims {
//     #[serde(rename = "iss")]
//     issuer: Option<String>,
//     #[serde(rename = "aud")]
//     audience: Option<String>,
//     #[serde(rename = "exp")]
//     expiration: Option<i64>,
// }

/// Resolves a DID and extracts the atproto signing key as a did:key string
async fn resolve_signing_key(
    resolver: &CommonDidResolver<ReqwestClient>,
    did_str: &str,
) -> Result<String> {
    debug!("Resolving DID: {}", did_str);

    // Convert string to Did type
    let did = did_str.parse().map_err(|e| {
        warn!("Invalid DID format: {}", e);
        anyhow!("Invalid DID format: {}", e)
    })?;

    // Resolve the DID document
    let did_doc = resolver.resolve(&did).await.map_err(|e| {
        warn!("Failed to resolve DID {}: {}", did_str, e);
        anyhow!("Failed to resolve DID: {}", e)
    })?;

    debug!("DID document resolved: {:?}", did_doc);

    // Use the built-in helper to get the signing key
    let verification_method = did_doc.get_signing_key().ok_or_else(|| {
        warn!("No atproto verification method found in DID document");
        anyhow!("No atproto signing key found in DID document")
    })?;

    debug!("Found verification method: {:?}", verification_method);

    // Extract publicKeyMultibase
    let public_key_multibase = verification_method
        .public_key_multibase
        .as_ref()
        .ok_or_else(|| {
            warn!("Verification method missing publicKeyMultibase");
            anyhow!("Missing publicKeyMultibase in verification method")
        })?;

    debug!("Public key multibase: {}", public_key_multibase);

    // Parse the multibase-encoded key
    let (algorithm, key_bytes) = parse_multikey(public_key_multibase).map_err(|e| {
        warn!("Failed to parse multikey: {}", e);
        anyhow!("Invalid publicKeyMultibase format: {}", e)
    })?;

    debug!(
        "Parsed key: algorithm={:?}, key_len={}",
        algorithm,
        key_bytes.len()
    );

    // Format as did:key
    let did_key = format_did_key(algorithm, &key_bytes).map_err(|e| {
        warn!("Failed to format did:key: {}", e);
        anyhow!("Failed to convert key to did:key format: {}", e)
    })?;

    debug!("Formatted did:key: {}", did_key);
    Ok(did_key)
}

pub async fn validate_jwt(token: &str, service_did: &str) -> Result<JwtClaims> {
    // Token should already have "Bearer " prefix stripped by caller
    debug!("Validating JWT token (length: {})", token.len());
    debug!("Expected audience: {}", service_did);

    // Parse the untrusted token to extract claims without verification
    let untrusted = UntrustedToken::new(token).map_err(|e| {
        warn!("Failed to parse JWT: {}", e);
        anyhow!("Invalid JWT format: {}", e)
    })?;

    // First, try to deserialize as raw JSON to see the actual structure
    let claims_wrapper = untrusted
        .deserialize_claims_unchecked::<serde_json::Value>()
        .map_err(|e| {
            warn!("Failed to deserialize JWT claims: {}", e);
            anyhow!("Invalid JWT claims: {}", e)
        })?;

    debug!("Raw JWT claims: {:?}", claims_wrapper);

    // Extract the actual claims from the Value
    let iss = claims_wrapper
        .custom
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing 'iss' claim"))?
        .to_string();

    let aud = claims_wrapper
        .custom
        .get("aud")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing 'aud' claim"))?
        .to_string();

    let exp = claims_wrapper
        .custom
        .get("exp")
        .and_then(|v| v.as_i64())
        .or_else(|| claims_wrapper.expiration.map(|ts| ts.timestamp()))
        .ok_or_else(|| anyhow!("Missing 'exp' claim"))?;

    debug!(
        "JWT claims extracted - issuer: {}, audience: {}, exp: {}",
        iss, aud, exp
    );

    // Validate audience
    if aud != service_did {
        warn!(
            "JWT audience mismatch: expected {}, got {}",
            service_did, aud
        );
        return Err(anyhow!("Invalid JWT audience"));
    }

    // Validate expiration
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    if exp < now {
        warn!("JWT expired: exp={}, now={}", exp, now);
        return Err(anyhow!("JWT has expired"));
    }

    // Verify signature
    debug!("Verifying JWT signature for issuer: {}", iss);

    // Create DID resolver
    // Note: base_uri is not used for DID resolution, so we use a placeholder
    let http_client = ReqwestClient::new("https://plc.directory");
    let resolver_config = CommonDidResolverConfig {
        plc_directory_url: DEFAULT_PLC_DIRECTORY_URL.to_string(),
        http_client: Arc::new(http_client),
    };
    let resolver = CommonDidResolver::new(resolver_config);

    // Resolve the issuer's signing key
    let did_key = resolve_signing_key(&resolver, &iss).await?;

    // Extract the signed portion of the JWT (header.payload)
    // JWT format is: header.payload.signature
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        warn!("Invalid JWT format: expected 3 parts, got {}", parts.len());
        return Err(anyhow!("Invalid JWT format"));
    }

    let signed_data = format!("{}.{}", parts[0], parts[1]);
    let signature_b64 = parts[2];

    // Decode the base64url signature
    let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|e| {
            warn!("Failed to decode JWT signature: {}", e);
            anyhow!("Invalid JWT signature encoding: {}", e)
        })?;

    // Verify the signature
    atrium_crypto::verify::verify_signature(&did_key, signed_data.as_bytes(), &signature_bytes)
        .map_err(|e| {
            warn!("JWT signature verification failed: {}", e);
            anyhow!("Invalid JWT signature: {}", e)
        })?;

    debug!("JWT signature verified successfully for issuer: {}", iss);
    Ok(JwtClaims { iss, aud, exp })
}
