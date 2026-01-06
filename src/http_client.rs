use anyhow::{anyhow, Result};
use reqwest::{Client, StatusCode};
use std::time::Duration;
use tracing::warn;

/// Maximum number of retry attempts
const MAX_RETRIES: u32 = 3;
/// Initial retry delay in milliseconds
const INITIAL_RETRY_MS: u64 = 500;
/// Request timeout in seconds
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Create a configured HTTP client with timeout
pub fn create_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))
}

/// Fetch JSON from a URL with retry logic for transient errors
pub async fn fetch_json_with_retry(
    client: &Client,
    url: &str,
) -> Result<serde_json::Value> {
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        match client.get(url).send().await {
            Ok(response) => {
                let status = response.status();

                // Check for rate limiting or server errors that are retryable
                if status == StatusCode::TOO_MANY_REQUESTS {
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(5);

                    warn!(
                        "Rate limited on {}, waiting {} seconds (attempt {}/{})",
                        url, retry_after, attempt + 1, MAX_RETRIES
                    );
                    tokio::time::sleep(Duration::from_secs(retry_after)).await;
                    continue;
                }

                if status.is_server_error() {
                    let delay = get_retry_delay(attempt);
                    warn!(
                        "Server error {} on {}, retrying in {:?} (attempt {}/{})",
                        status, url, delay, attempt + 1, MAX_RETRIES
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(anyhow!("Server error: {}", status));
                    continue;
                }

                if !status.is_success() {
                    // Log the response body for debugging
                    let body = response.text().await.unwrap_or_default();
                    warn!(
                        "Request to {} failed with status {}: {}",
                        url, status, body
                    );
                    return Err(anyhow!("Request failed with status {}: {}", status, body));
                }

                // Try to parse JSON response
                match response.json::<serde_json::Value>().await {
                    Ok(json) => return Ok(json),
                    Err(e) => {
                        warn!(
                            "Failed to parse JSON from {}: {} (attempt {}/{})",
                            url, e, attempt + 1, MAX_RETRIES
                        );
                        last_error = Some(anyhow!("JSON parse error: {}", e));

                        // Only retry on parse errors if it might be transient
                        if attempt < MAX_RETRIES - 1 {
                            let delay = get_retry_delay(attempt);
                            tokio::time::sleep(delay).await;
                            continue;
                        }
                    }
                }
            }
            Err(e) => {
                // Network errors are retryable
                if e.is_timeout() || e.is_connect() {
                    let delay = get_retry_delay(attempt);
                    warn!(
                        "Network error on {}: {}, retrying in {:?} (attempt {}/{})",
                        url, e, delay, attempt + 1, MAX_RETRIES
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(anyhow!("Network error: {}", e));
                    continue;
                }

                // Non-retryable request errors
                return Err(anyhow!("Request error: {}", e));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("Request failed after {} attempts", MAX_RETRIES)))
}

/// Get exponential backoff delay for retry attempt
fn get_retry_delay(attempt: u32) -> Duration {
    let delay_ms = INITIAL_RETRY_MS * 2u64.pow(attempt);
    Duration::from_millis(delay_ms.min(10000)) // Cap at 10 seconds
}
