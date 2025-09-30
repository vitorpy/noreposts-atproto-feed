use anyhow::{anyhow, Result};
use jwt_compact::UntrustedToken;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::types::JwtClaims;

// Empty custom claims - all required fields are in standard JWT claims
#[derive(Debug, Deserialize)]
struct EmptyCustomClaims {}

// Standard JWT claims structure as expected by jwt-compact
#[derive(Debug, Deserialize)]
struct StandardClaims {
    #[serde(rename = "iss")]
    issuer: Option<String>,
    #[serde(rename = "aud")]
    audience: Option<String>,
    #[serde(rename = "exp")]
    expiration: Option<i64>,
}

pub fn validate_jwt(token: &str, service_did: &str) -> Result<JwtClaims> {
    // Token should already have "Bearer " prefix stripped by caller
    debug!("Validating JWT token (length: {})", token.len());
    debug!("Expected audience: {}", service_did);

    // Parse the untrusted token to extract claims without verification
    let untrusted = UntrustedToken::new(token)
        .map_err(|e| {
            warn!("Failed to parse JWT: {}", e);
            anyhow!("Invalid JWT format: {}", e)
        })?;

    // First, try to deserialize as raw JSON to see the actual structure
    let claims_wrapper = untrusted.deserialize_claims_unchecked::<serde_json::Value>()
        .map_err(|e| {
            warn!("Failed to deserialize JWT claims: {}", e);
            anyhow!("Invalid JWT claims: {}", e)
        })?;

    debug!("Raw JWT claims: {:?}", claims_wrapper);

    // Extract the actual claims from the Value
    let iss = claims_wrapper.custom.get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing 'iss' claim"))?
        .to_string();

    let aud = claims_wrapper.custom.get("aud")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing 'aud' claim"))?
        .to_string();

    let exp = claims_wrapper.custom.get("exp")
        .and_then(|v| v.as_i64())
        .or_else(|| claims_wrapper.expiration.map(|ts| ts.timestamp()))
        .ok_or_else(|| anyhow!("Missing 'exp' claim"))?;

    debug!("JWT claims extracted - issuer: {}, audience: {}, exp: {}", iss, aud, exp);

    // Validate audience
    if aud != service_did {
        warn!("JWT audience mismatch: expected {}, got {}", service_did, aud);
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

    // TODO: In production, we should:
    // 1. Fetch the user's DID document from iss
    // 2. Extract their public key
    // 3. Verify the signature with Es256k::verify()
    // For now, we skip signature verification but validate structure and claims

    debug!("JWT validated successfully for issuer: {}", iss);
    Ok(JwtClaims { iss, aud, exp })
}

// Production implementation would need this:
/*
pub async fn validate_jwt_production(auth_header: &str, service_did: &str) -> Result<JwtClaims> {
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow!("Invalid authorization header format"))?;

    // 1. Decode JWT header to get the signing key ID
    let header = decode_header(token)?;
    
    // 2. Extract issuer DID from token payload (without verification)
    let mut validation = Validation::new(Algorithm::ES256K);
    validation.insecure_disable_signature_validation();
    let temp_decode = decode::<JwtClaims>(token, &DecodingKey::from_secret(b"temp"), &validation)?;
    let issuer_did = temp_decode.claims.iss;
    
    // 3. Fetch DID document for the issuer
    let did_doc = fetch_did_document(&issuer_did).await?;
    
    // 4. Extract the appropriate verification key
    let verification_key = extract_verification_key(&did_doc, &header.kid)?;
    
    // 5. Validate the JWT with the real key
    let mut validation = Validation::new(Algorithm::ES256K);
    validation.validate_exp = true;
    validation.set_audience(&[service_did]);
    
    let decoding_key = DecodingKey::from_ec_pem(&verification_key)?;
    let token_data = decode::<JwtClaims>(token, &decoding_key, &validation)?;
    
    Ok(token_data.claims)
}
*/
