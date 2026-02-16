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

/// Update an existing request log entry with post-stream usage data.
///
/// Writes input_tokens, output_tokens, and cost_sats to the row matching
/// the given correlation_id. Returns the number of rows affected.
///
/// Per user decision: only updates token/cost columns. Latency stays as
/// TTFB from INSERT (Phase 10 handles full-stream latency).
pub async fn update_usage(
    pool: &SqlitePool,
    correlation_id: &str,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE requests SET input_tokens = ?, output_tokens = ?, cost_sats = ? WHERE correlation_id = ?",
    )
    .bind(input_tokens.map(|v| v as i64))
    .bind(output_tokens.map(|v| v as i64))
    .bind(cost_sats)
    .bind(correlation_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Spawn a fire-and-forget database usage update.
///
/// Warns if the update affects zero rows (row not found) or fails.
/// Logs at debug level on success.
pub fn spawn_usage_update(
    pool: &SqlitePool,
    correlation_id: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
) {
    let pool = pool.clone();
    tokio::spawn(async move {
        match update_usage(
            &pool,
            &correlation_id,
            input_tokens,
            output_tokens,
            cost_sats,
        )
        .await
        {
            Ok(0) => {
                tracing::warn!(
                    correlation_id = %correlation_id,
                    "Usage update affected zero rows"
                );
            }
            Ok(_) => {
                tracing::debug!(
                    correlation_id = %correlation_id,
                    "Updated request log with usage data"
                );
            }
            Err(e) => {
                tracing::warn!(
                    correlation_id = %correlation_id,
                    error = %e,
                    "Failed to update request log with usage data"
                );
            }
        }
    });
}

/// Update an existing request log entry with post-stream completion data.
///
/// Writes input_tokens, output_tokens, cost_sats, stream_duration_ms,
/// success, and error_message to the row matching the given correlation_id.
/// Returns the number of rows affected.
#[allow(clippy::too_many_arguments)]
pub async fn update_stream_completion(
    pool: &SqlitePool,
    correlation_id: &str,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
    stream_duration_ms: i64,
    success: bool,
    error_message: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE requests SET input_tokens = ?, output_tokens = ?, cost_sats = ?, stream_duration_ms = ?, success = ?, error_message = ? WHERE correlation_id = ?",
    )
    .bind(input_tokens.map(|v| v as i64))
    .bind(output_tokens.map(|v| v as i64))
    .bind(cost_sats)
    .bind(stream_duration_ms)
    .bind(success)
    .bind(error_message)
    .bind(correlation_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Spawn a fire-and-forget database stream completion update.
///
/// Warns if the update affects zero rows (row not found) or fails.
/// Logs at debug level on success.
#[allow(clippy::too_many_arguments)]
pub fn spawn_stream_completion_update(
    pool: &SqlitePool,
    correlation_id: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
    stream_duration_ms: i64,
    success: bool,
    error_message: Option<String>,
) {
    let pool = pool.clone();
    tokio::spawn(async move {
        match update_stream_completion(
            &pool,
            &correlation_id,
            input_tokens,
            output_tokens,
            cost_sats,
            stream_duration_ms,
            success,
            error_message.as_deref(),
        )
        .await
        {
            Ok(0) => {
                tracing::warn!(
                    correlation_id = %correlation_id,
                    "Stream completion update affected zero rows"
                );
            }
            Ok(_) => {
                tracing::debug!(
                    correlation_id = %correlation_id,
                    "Updated request log with stream completion data"
                );
            }
            Err(e) => {
                tracing::warn!(
                    correlation_id = %correlation_id,
                    error = %e,
                    "Failed to update request log with stream completion data"
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create an in-memory SQLite pool with migrations applied.
    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    /// Helper: insert a test row and return its correlation_id.
    async fn insert_test_row(pool: &SqlitePool, correlation_id: &str) {
        let log = RequestLog {
            correlation_id: correlation_id.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            model: "gpt-4o".to_string(),
            provider: Some("test-provider".to_string()),
            policy: None,
            streaming: true,
            input_tokens: None,
            output_tokens: None,
            cost_sats: None,
            provider_cost_sats: None,
            latency_ms: 100,
            success: true,
            error_status: None,
            error_message: None,
        };
        log.insert(pool).await.unwrap();
    }

    #[tokio::test]
    async fn update_usage_writes_tokens() {
        let pool = test_pool().await;
        let cid = "test-update-001";
        insert_test_row(&pool, cid).await;

        let rows = update_usage(&pool, cid, Some(150), Some(300), Some(42.5))
            .await
            .unwrap();
        assert_eq!(rows, 1);

        // Verify the values were written
        let row: (Option<i64>, Option<i64>, Option<f64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, cost_sats FROM requests WHERE correlation_id = ?",
        )
        .bind(cid)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.0, Some(150));
        assert_eq!(row.1, Some(300));
        assert!((row.2.unwrap() - 42.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn update_usage_with_nulls() {
        let pool = test_pool().await;
        let cid = "test-update-002";
        insert_test_row(&pool, cid).await;

        let rows = update_usage(&pool, cid, None, None, None).await.unwrap();
        assert_eq!(rows, 1);

        // Verify the values are NULL
        let row: (Option<i64>, Option<i64>, Option<f64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, cost_sats FROM requests WHERE correlation_id = ?",
        )
        .bind(cid)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert!(row.0.is_none());
        assert!(row.1.is_none());
        assert!(row.2.is_none());
    }

    #[tokio::test]
    async fn update_usage_no_matching_row() {
        let pool = test_pool().await;

        let rows = update_usage(&pool, "nonexistent-id", Some(100), Some(200), Some(10.0))
            .await
            .unwrap();
        assert_eq!(
            rows, 0,
            "Should affect zero rows for non-existent correlation_id"
        );
    }

    #[tokio::test]
    async fn test_update_stream_completion_writes_all_fields() {
        let pool = test_pool().await;
        let cid = "test-stream-complete-001";
        insert_test_row(&pool, cid).await;

        let rows = update_stream_completion(
            &pool,
            cid,
            Some(150),
            Some(300),
            Some(42.5),
            2500,
            true,
            None,
        )
        .await
        .unwrap();
        assert_eq!(rows, 1);

        // Verify all 6 columns were set correctly
        let row: (Option<i64>, Option<i64>, Option<f64>, Option<i64>, bool, Option<String>) =
            sqlx::query_as(
                "SELECT input_tokens, output_tokens, cost_sats, stream_duration_ms, success, error_message FROM requests WHERE correlation_id = ?",
            )
            .bind(cid)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(row.0, Some(150));
        assert_eq!(row.1, Some(300));
        assert!((row.2.unwrap() - 42.5).abs() < f64::EPSILON);
        assert_eq!(row.3, Some(2500));
        assert!(row.4);
        assert!(row.5.is_none());
    }

    #[tokio::test]
    async fn test_update_stream_completion_null_tokens() {
        let pool = test_pool().await;
        let cid = "test-stream-complete-002";
        insert_test_row(&pool, cid).await;

        let rows = update_stream_completion(
            &pool,
            cid,
            None,
            None,
            None,
            1800,
            true,
            Some("client_disconnected"),
        )
        .await
        .unwrap();
        assert_eq!(rows, 1);

        let row: (Option<i64>, Option<i64>, Option<f64>, Option<i64>, bool, Option<String>) =
            sqlx::query_as(
                "SELECT input_tokens, output_tokens, cost_sats, stream_duration_ms, success, error_message FROM requests WHERE correlation_id = ?",
            )
            .bind(cid)
            .fetch_one(&pool)
            .await
            .unwrap();

        assert!(row.0.is_none());
        assert!(row.1.is_none());
        assert!(row.2.is_none());
        assert_eq!(row.3, Some(1800));
        assert!(row.4);
        assert_eq!(row.5.as_deref(), Some("client_disconnected"));
    }
}
