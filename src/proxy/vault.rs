//! Vault treasury integration for per-request billing.
//!
//! Implements the reserve/settle/release pattern against an arbstr vault
//! instance. When configured, every inference request:
//!
//! ```text
//! resolve_candidates() → reserve(estimated_cost) → route to provider
//!                      → settle(actual_cost)  [on success]
//!                      → release()            [on failure]
//! ```
//!
//! When vault is not configured, all methods are no-ops and arbstr runs
//! in free proxy mode.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::config::VaultConfig;

/// Retry configuration for settle/release calls.
const MAX_RETRIES: u32 = 3;
const RETRY_BASE_MS: u64 = 100;

/// Timeout for vault HTTP calls.
const VAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A reservation returned by the vault on successful reserve.
#[derive(Debug, Clone)]
pub struct Reservation {
    pub id: String,
    pub reserved_msats: u64,
}

/// Metadata sent with a settle call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettleMetadata {
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    pub provider: String,
    pub latency_ms: i64,
}

/// Response from a successful settle.
#[derive(Debug, Clone, Deserialize)]
pub struct SettleResponse {
    pub settled: bool,
    pub refunded_msats: Option<u64>,
}

/// Errors from vault API calls.
#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    /// Agent has insufficient balance (HTTP 402).
    #[error("Insufficient balance")]
    InsufficientBalance,

    /// Agent's policy denied the request (HTTP 403).
    #[error("Policy denied: {0}")]
    PolicyDenied(String),

    /// Rate limited by vault (HTTP 429).
    #[error("Vault rate limited")]
    RateLimited,

    /// Vault service is unreachable or returned a server error.
    #[error("Vault unavailable: {0}")]
    Unavailable(String),

    /// Unexpected response from vault.
    #[error("Vault error: {0}")]
    Other(String),
}

impl VaultError {
    /// Map a vault error to an HTTP status code for the client.
    pub fn status_code(&self) -> u16 {
        match self {
            VaultError::InsufficientBalance => 402,
            VaultError::PolicyDenied(_) => 403,
            VaultError::RateLimited => 429,
            VaultError::Unavailable(_) => 503,
            VaultError::Other(_) => 500,
        }
    }
}

/// Client for the arbstr vault internal API.
///
/// Handles reserve/settle/release calls with retry logic.
/// Pending settlement writes go through direct sqlx (not DbWriter)
/// to avoid double-silent-failure.
#[derive(Clone)]
pub struct VaultClient {
    client: Client,
    base_url: String,
    token: String,
    pub default_reserve_tokens: u32,
    pub pending_threshold: u32,
    /// Set to true when pending settlements exceed threshold.
    pub backpressure: Arc<AtomicBool>,
}

impl VaultClient {
    /// Create a new vault client from config, reusing an existing reqwest client.
    pub fn new(client: Client, config: &VaultConfig) -> Self {
        Self {
            client,
            base_url: config.url.trim_end_matches('/').to_string(),
            token: config.internal_token.expose_secret().to_string(),
            default_reserve_tokens: config.default_reserve_tokens,
            pending_threshold: config.pending_threshold,
            backpressure: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Check if backpressure is active (too many pending settlements).
    pub fn is_backpressured(&self) -> bool {
        self.backpressure.load(Ordering::Relaxed)
    }

    /// Reserve funds from a buyer's vault account before routing.
    ///
    /// The `agent_token` is the client's bearer token, forwarded to vault
    /// for authentication and agent identification. Vault validates it and
    /// returns a reservation_id.
    pub async fn reserve(
        &self,
        agent_token: &str,
        amount_msats: u64,
        correlation_id: &str,
        model: &str,
    ) -> Result<Reservation, VaultError> {
        let url = format!("{}/internal/reserve", self.base_url);

        let body = serde_json::json!({
            "agent_token": agent_token,
            "amount_msats": amount_msats,
            "correlation_id": correlation_id,
            "model": model,
        });

        let response = self
            .client
            .post(&url)
            .header("X-Internal-Token", &self.token)
            .timeout(VAULT_TIMEOUT)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() || e.is_connect() {
                    VaultError::Unavailable(format!("Connection failed: {}", e))
                } else {
                    VaultError::Other(e.to_string())
                }
            })?;

        let status = response.status().as_u16();
        match status {
            200..=299 => {
                let resp: serde_json::Value = response.json().await.map_err(|e| {
                    VaultError::Other(format!("Invalid reserve response: {}", e))
                })?;
                let id = resp
                    .get("reservation_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        VaultError::Other("Missing reservation_id in response".to_string())
                    })?
                    .to_string();
                Ok(Reservation {
                    id,
                    reserved_msats: amount_msats,
                })
            }
            402 => Err(VaultError::InsufficientBalance),
            403 => {
                let body = response.text().await.unwrap_or_default();
                Err(VaultError::PolicyDenied(body))
            }
            429 => Err(VaultError::RateLimited),
            500..=599 => {
                let body = response.text().await.unwrap_or_default();
                Err(VaultError::Unavailable(format!("HTTP {}: {}", status, body)))
            }
            _ => {
                let body = response.text().await.unwrap_or_default();
                Err(VaultError::Other(format!("HTTP {}: {}", status, body)))
            }
        }
    }

    /// Settle a reservation after successful inference.
    ///
    /// Retries up to MAX_RETRIES times with exponential backoff.
    /// Returns the settle response on success, or the last error on failure.
    pub async fn settle(
        &self,
        reservation_id: &str,
        actual_msats: u64,
        metadata: SettleMetadata,
    ) -> Result<SettleResponse, VaultError> {
        let url = format!("{}/internal/settle", self.base_url);

        let body = serde_json::json!({
            "reservation_id": reservation_id,
            "actual_msats": actual_msats,
            "metadata": metadata,
        });

        self.call_with_retry(&url, &body).await.and_then(|resp| {
            serde_json::from_value::<SettleResponse>(resp)
                .map_err(|e| VaultError::Other(format!("Invalid settle response: {}", e)))
        })
    }

    /// Release a reservation (refund buyer) after failed inference.
    ///
    /// Retries up to MAX_RETRIES times with exponential backoff.
    pub async fn release(
        &self,
        reservation_id: &str,
        reason: &str,
    ) -> Result<(), VaultError> {
        let url = format!("{}/internal/release", self.base_url);

        let body = serde_json::json!({
            "reservation_id": reservation_id,
            "reason": reason,
        });

        self.call_with_retry(&url, &body).await.map(|_| ())
    }

    /// POST to a vault endpoint with retry logic.
    async fn call_with_retry(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, VaultError> {
        let mut last_err = VaultError::Other("no attempts made".to_string());

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(RETRY_BASE_MS * 2u64.pow(attempt));
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(url)
                .header("X-Internal-Token", &self.token)
                .timeout(VAULT_TIMEOUT)
                .json(body)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status().as_u16();
                    match status {
                        200..=299 => {
                            return response.json().await.map_err(|e| {
                                VaultError::Other(format!("Invalid response body: {}", e))
                            });
                        }
                        500..=599 => {
                            let msg = response.text().await.unwrap_or_default();
                            last_err =
                                VaultError::Unavailable(format!("HTTP {}: {}", status, msg));
                            tracing::warn!(
                                attempt = attempt + 1,
                                url,
                                status,
                                "Vault call failed, retrying"
                            );
                        }
                        _ => {
                            let msg = response.text().await.unwrap_or_default();
                            return Err(VaultError::Other(format!("HTTP {}: {}", status, msg)));
                        }
                    }
                }
                Err(e) => {
                    last_err = if e.is_timeout() || e.is_connect() {
                        VaultError::Unavailable(format!("Connection failed: {}", e))
                    } else {
                        VaultError::Other(e.to_string())
                    };
                    tracing::warn!(
                        attempt = attempt + 1,
                        url,
                        error = %e,
                        "Vault call failed, retrying"
                    );
                }
            }
        }

        Err(last_err)
    }
}

/// Estimate the reserve amount in millisatoshis for a request.
///
/// Uses the cheapest candidate's rates and the request's token estimate.
/// If `max_tokens` is set, uses that as the output ceiling.
/// Otherwise uses `default_reserve_tokens` from vault config.
pub fn estimate_reserve_msats(
    estimated_input_tokens: u32,
    estimated_output_tokens: u32,
    input_rate: u64,
    output_rate: u64,
    base_fee: u64,
) -> u64 {
    // Rates are in sats per 1000 tokens. Convert to msats.
    let input_cost_msats =
        (estimated_input_tokens as u64 * input_rate * 1000) / 1000;
    let output_cost_msats =
        (estimated_output_tokens as u64 * output_rate * 1000) / 1000;
    let base_fee_msats = base_fee * 1000;
    input_cost_msats + output_cost_msats + base_fee_msats
}

/// A pending settlement record for when vault is unreachable.
///
/// Stored in SQLite and replayed by the reconciliation task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingSettlement {
    /// "settle" or "release"
    pub settlement_type: String,
    pub reservation_id: String,
    /// Millisatoshis (for settle), None for release.
    pub amount_msats: Option<u64>,
    /// JSON-encoded metadata (SettleMetadata for settle, reason for release).
    pub metadata: String,
}

/// Maximum number of replay attempts before a pending settlement is evicted.
const MAX_SETTLEMENT_ATTEMPTS: i64 = 10;

// ── Pending settlement persistence (direct sqlx, not DbWriter) ──

/// Insert a pending settlement into SQLite. Uses direct sqlx to guarantee
/// persistence even under backpressure (not routed through DbWriter channel).
pub async fn insert_pending_settlement(
    pool: &sqlx::SqlitePool,
    settlement: &PendingSettlement,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO pending_settlements (type, reservation_id, amount_msats, metadata, created_at, attempts)
         VALUES (?, ?, ?, ?, ?, 0)",
    )
    .bind(&settlement.settlement_type)
    .bind(&settlement.reservation_id)
    .bind(settlement.amount_msats.map(|v| v as i64))
    .bind(&settlement.metadata)
    .bind(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    .execute(pool)
    .await?;
    Ok(())
}

/// Count pending settlements.
pub async fn count_pending(pool: &sqlx::SqlitePool) -> Result<i64, sqlx::Error> {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM pending_settlements")
            .fetch_one(pool)
            .await?;
    Ok(count)
}

/// Fetch all pending settlements for replay.
///
/// Returns `(id, PendingSettlement, attempts)` tuples ordered by creation time.
pub async fn fetch_pending(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<(i64, PendingSettlement, i64)>, sqlx::Error> {
    let rows: Vec<(i64, String, String, Option<i64>, String, i64)> = sqlx::query_as(
        "SELECT id, type, reservation_id, amount_msats, metadata, attempts FROM pending_settlements ORDER BY created_at ASC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, typ, rid, amt, meta, attempts)| {
            (
                id,
                PendingSettlement {
                    settlement_type: typ,
                    reservation_id: rid,
                    amount_msats: amt.map(|v| v as u64),
                    metadata: meta,
                },
                attempts,
            )
        })
        .collect())
}

/// Delete a pending settlement after successful replay or eviction.
pub async fn delete_pending(pool: &sqlx::SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM pending_settlements WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Increment the attempt counter for a pending settlement.
async fn increment_attempts(pool: &sqlx::SqlitePool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE pending_settlements SET attempts = attempts + 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Replay a single pending settlement against vault.
async fn replay_one(
    vault: &VaultClient,
    settlement: &PendingSettlement,
) -> Result<(), VaultError> {
    match settlement.settlement_type.as_str() {
        "settle" => {
            let metadata: SettleMetadata =
                serde_json::from_str(&settlement.metadata).map_err(|e| {
                    VaultError::Other(format!("Failed to parse settle metadata: {}", e))
                })?;
            vault
                .settle(
                    &settlement.reservation_id,
                    settlement.amount_msats.unwrap_or(0),
                    metadata,
                )
                .await?;
            Ok(())
        }
        "release" => {
            vault
                .release(&settlement.reservation_id, &settlement.metadata)
                .await
        }
        other => Err(VaultError::Other(format!(
            "Unknown settlement type: {}",
            other
        ))),
    }
}

/// Background reconciliation task.
///
/// Runs in a loop every `interval`, replaying pending settlements against vault.
/// Updates the backpressure flag on the VaultClient based on pending count.
/// Stops when the cancellation token is triggered (graceful shutdown).
pub async fn reconciliation_loop(
    vault: VaultClient,
    pool: sqlx::SqlitePool,
    interval: Duration,
    cancel: tokio::sync::watch::Receiver<bool>,
) {
    // Initial replay (non-blocking, runs in background)
    let _ = reconcile_once(&vault, &pool).await;

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let _ = reconcile_once(&vault, &pool).await;
            }
            _ = cancel_wait(&cancel) => {
                tracing::info!("Reconciliation task shutting down");
                // Final reconciliation attempt before exit
                let _ = reconcile_once(&vault, &pool).await;
                break;
            }
        }
    }
}

/// Wait for the cancellation signal.
async fn cancel_wait(cancel: &tokio::sync::watch::Receiver<bool>) {
    let mut cancel = cancel.clone();
    // Wait until the value becomes true
    while !*cancel.borrow_and_update() {
        if cancel.changed().await.is_err() {
            return; // Sender dropped
        }
    }
}

/// Run one reconciliation pass.
///
/// Returns `(replayed, failed, evicted)` counts for observability and testing.
/// Settlements with `attempts >= MAX_SETTLEMENT_ATTEMPTS` are evicted (deleted)
/// without replay, logged at error level.
pub async fn reconcile_once(vault: &VaultClient, pool: &sqlx::SqlitePool) -> (u32, u32, u32) {
    // Update backpressure flag
    match count_pending(pool).await {
        Ok(count) => {
            let over_threshold = count >= vault.pending_threshold as i64;
            let was_backpressured = vault.backpressure.load(std::sync::atomic::Ordering::Relaxed);
            vault
                .backpressure
                .store(over_threshold, std::sync::atomic::Ordering::Relaxed);
            if over_threshold && !was_backpressured {
                tracing::warn!(
                    count = count,
                    threshold = vault.pending_threshold,
                    "Vault backpressure activated"
                );
            } else if !over_threshold && was_backpressured {
                tracing::info!("Vault backpressure cleared");
            }

            if count == 0 {
                return (0, 0, 0);
            }
            tracing::info!(count = count, "Replaying pending settlements");
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to count pending settlements");
            return (0, 0, 0);
        }
    }

    // Fetch and replay
    let pending = match fetch_pending(pool).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "Failed to fetch pending settlements");
            return (0, 0, 0);
        }
    };

    let mut replayed = 0u32;
    let mut failed = 0u32;
    let mut evicted = 0u32;

    for (id, settlement, attempts) in &pending {
        // Evict stale settlements that have exceeded max retry attempts
        if *attempts >= MAX_SETTLEMENT_ATTEMPTS {
            tracing::error!(
                reservation_id = %settlement.reservation_id,
                settlement_type = %settlement.settlement_type,
                attempts = attempts,
                "Evicting stale pending settlement after max attempts"
            );
            if let Err(e) = delete_pending(pool, *id).await {
                tracing::error!(id = id, error = %e, "Failed to delete evicted settlement");
            } else {
                evicted += 1;
            }
            continue;
        }

        match replay_one(vault, settlement).await {
            Ok(()) => {
                if let Err(e) = delete_pending(pool, *id).await {
                    tracing::error!(id = id, error = %e, "Failed to delete replayed settlement");
                } else {
                    replayed += 1;
                }
            }
            Err(e) => {
                tracing::warn!(
                    id = id,
                    reservation_id = %settlement.reservation_id,
                    error = %e,
                    "Failed to replay pending settlement"
                );
                let _ = increment_attempts(pool, *id).await;
                failed += 1;
            }
        }
    }

    if replayed > 0 || failed > 0 || evicted > 0 {
        tracing::info!(replayed = replayed, failed = failed, evicted = evicted, "Reconciliation pass complete");
    }

    (replayed, failed, evicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_reserve_msats() {
        // 100 input tokens * 10 sats/1k = 1 sat = 1000 msats
        // 200 output tokens * 30 sats/1k = 6 sats = 6000 msats
        // base_fee 1 sat = 1000 msats
        // total = 8000 msats
        let result = estimate_reserve_msats(100, 200, 10, 30, 1);
        assert_eq!(result, 8000);
    }

    #[test]
    fn test_estimate_reserve_zero_tokens() {
        // base_fee only
        let result = estimate_reserve_msats(0, 0, 10, 30, 5);
        assert_eq!(result, 5000);
    }

    #[test]
    fn test_estimate_reserve_large_request() {
        // 1000 input * 10/1k = 10 sats = 10000 msats
        // 4096 output * 30/1k = 122.88 sats → 122880 msats (integer truncation)
        // base_fee 0
        let result = estimate_reserve_msats(1000, 4096, 10, 30, 0);
        assert_eq!(result, 10000 + 122880);
    }

    #[test]
    fn test_estimate_reserve_frontier_rates() {
        // Frontier rates: input=10, output=30, base=2
        // 100 input * 10/1k = 1 sat = 1000 msats
        // 4096 output * 30/1k = 122.88 sats = 122880 msats
        // base_fee 2 sats = 2000 msats
        // total = 125880 msats
        let result = estimate_reserve_msats(100, 4096, 10, 30, 2);
        assert_eq!(result, 125880);
    }

    #[test]
    fn test_vault_error_status_codes() {
        assert_eq!(VaultError::InsufficientBalance.status_code(), 402);
        assert_eq!(VaultError::PolicyDenied("x".into()).status_code(), 403);
        assert_eq!(VaultError::RateLimited.status_code(), 429);
        assert_eq!(VaultError::Unavailable("x".into()).status_code(), 503);
        assert_eq!(VaultError::Other("x".into()).status_code(), 500);
    }
}
