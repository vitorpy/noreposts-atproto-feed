use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

use crate::types::JwtClaims;

pub fn validate_jwt(auth_header: &str, service_did: &str) -> Result<JwtClaims> {
    // Extract bearer token
    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or_else(|| anyhow!("Invalid authorization header format"))?;

    // For this example, we'll use a simplified JWT validation
    // In production, you'd need to:
    // 1. Fetch the user's DID document
    // 2. Extract their signing key
    // 3. Validate the signature with that key
    
    // For now, let's decode without verification (unsafe for production!)
    let mut validation = Validation::new(Algorithm::ES256);
    validation.insecure_disable_signature_validation();
    validation.validate_exp = true;
    validation.set_audience(&[service_did]);

    // This is a placeholder - in production you need the actual signing key
    let decoding_key = DecodingKey::from_secret(b"placeholder");
    
    let token_data = decode::<JwtClaims>(token, &decoding_key, &validation)
        .map_err(|e| anyhow!("JWT validation failed: {}", e))?;

    Ok(token_data.claims)
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
