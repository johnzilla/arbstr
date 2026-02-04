//! Request logging data types and database operations.

use sqlx::SqlitePool;

/// A completed request log entry ready for database insertion.
///
/// All fields are owned types to satisfy `tokio::spawn` `'static` requirement.
pub struct RequestLog {
    pub correlation_id: String,
    pub timestamp: String,
    pub model: String,
    pub provider: Option<String>,
    pub policy: Option<String>,
    pub streaming: bool,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cost_sats: Option<f64>,
    pub provider_cost_sats: Option<f64>,
    pub latency_ms: i64,
    pub success: bool,
    pub error_status: Option<u16>,
    pub error_message: Option<String>,
}

impl RequestLog {
    /// Insert this log entry into the database.
    pub async fn insert(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO requests (
                correlation_id, timestamp, model, provider, policy,
                streaming, input_tokens, output_tokens,
                cost_sats, provider_cost_sats,
                latency_ms, success, error_status, error_message
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&self.correlation_id)
        .bind(&self.timestamp)
        .bind(&self.model)
        .bind(&self.provider)
        .bind(&self.policy)
        .bind(self.streaming)
        .bind(self.input_tokens.map(|v| v as i64))
        .bind(self.output_tokens.map(|v| v as i64))
        .bind(self.cost_sats)
        .bind(self.provider_cost_sats)
        .bind(self.latency_ms)
        .bind(self.success)
        .bind(self.error_status.map(|v| v as i32))
        .bind(self.error_message.as_deref())
        .execute(pool)
        .await?;
        Ok(())
    }
}

/// Spawn a fire-and-forget database write.
///
/// If the write fails, a warning is logged but the error is not propagated.
pub fn spawn_log_write(pool: &SqlitePool, log: RequestLog) {
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Err(e) = log.insert(&pool).await {
            tracing::warn!(
                correlation_id = %log.correlation_id,
                error = %e,
                "Failed to write request log to database"
            );
        }
    });
}
