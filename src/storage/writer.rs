//! Bounded channel-based database writer.
//!
//! Replaces fire-and-forget `tokio::spawn` writes with a bounded mpsc channel
//! and a dedicated writer task. This prevents unbounded queue growth under load
//! and provides backpressure when the channel fills up.

use sqlx::SqlitePool;
use tokio::sync::mpsc;

use super::logging::RequestLog;

/// Default channel capacity.
const DEFAULT_CAPACITY: usize = 1024;

/// Commands that the writer task processes.
enum WriteCommand {
    /// Insert a new request log row.
    Insert(RequestLog),
    /// Update usage data on an existing row.
    UpdateUsage {
        correlation_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        cost_sats: Option<f64>,
    },
    /// Update stream completion data on an existing row.
    UpdateStreamCompletion {
        correlation_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        cost_sats: Option<f64>,
        stream_duration_ms: i64,
        success: bool,
        error_message: Option<String>,
        complexity_score: Option<f64>,
        tier: Option<String>,
    },
}

/// A bounded, channel-based database writer.
///
/// Send writes through the channel; a dedicated background task processes
/// them sequentially. When the channel is full, `try_send` drops the write
/// and logs a warning instead of blocking the request path.
#[derive(Clone)]
pub struct DbWriter {
    tx: mpsc::Sender<WriteCommand>,
}

impl DbWriter {
    /// Spawn the writer task and return a `DbWriter` handle.
    pub fn new(pool: SqlitePool) -> Self {
        Self::with_capacity(pool, DEFAULT_CAPACITY)
    }

    /// Spawn the writer task with a custom channel capacity.
    pub fn with_capacity(pool: SqlitePool, capacity: usize) -> Self {
        let (tx, rx) = mpsc::channel(capacity);
        tokio::spawn(writer_loop(pool, rx));
        DbWriter { tx }
    }

    /// Queue a request log insert. Drops the write if the channel is full.
    pub fn log_write(&self, log: RequestLog) {
        if let Err(e) = self.tx.try_send(WriteCommand::Insert(log)) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    tracing::warn!("DB writer channel full, dropping log write");
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::warn!("DB writer channel closed, dropping log write");
                }
            }
        }
    }

    /// Queue a usage update. Drops the write if the channel is full.
    pub fn usage_update(
        &self,
        correlation_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        cost_sats: Option<f64>,
    ) {
        if let Err(e) = self.tx.try_send(WriteCommand::UpdateUsage {
            correlation_id,
            input_tokens,
            output_tokens,
            cost_sats,
        }) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    tracing::warn!("DB writer channel full, dropping usage update");
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::warn!("DB writer channel closed, dropping usage update");
                }
            }
        }
    }

    /// Queue a stream completion update. Drops the write if the channel is full.
    #[allow(clippy::too_many_arguments)]
    pub fn stream_completion_update(
        &self,
        correlation_id: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        cost_sats: Option<f64>,
        stream_duration_ms: i64,
        success: bool,
        error_message: Option<String>,
        complexity_score: Option<f64>,
        tier: Option<String>,
    ) {
        if let Err(e) = self.tx.try_send(WriteCommand::UpdateStreamCompletion {
            correlation_id,
            input_tokens,
            output_tokens,
            cost_sats,
            stream_duration_ms,
            success,
            error_message,
            complexity_score,
            tier,
        }) {
            match e {
                mpsc::error::TrySendError::Full(_) => {
                    tracing::warn!("DB writer channel full, dropping stream completion update");
                }
                mpsc::error::TrySendError::Closed(_) => {
                    tracing::warn!("DB writer channel closed, dropping stream completion update");
                }
            }
        }
    }
}

/// Background task that processes write commands sequentially.
async fn writer_loop(pool: SqlitePool, mut rx: mpsc::Receiver<WriteCommand>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::Insert(log) => {
                if let Err(e) = log.insert(&pool).await {
                    tracing::warn!(
                        correlation_id = %log.correlation_id,
                        error = %e,
                        "Failed to write request log to database"
                    );
                }
            }
            WriteCommand::UpdateUsage {
                correlation_id,
                input_tokens,
                output_tokens,
                cost_sats,
            } => {
                match super::logging::update_usage(
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
            }
            WriteCommand::UpdateStreamCompletion {
                correlation_id,
                input_tokens,
                output_tokens,
                cost_sats,
                stream_duration_ms,
                success,
                error_message,
                complexity_score,
                tier,
            } => {
                match super::logging::update_stream_completion(
                    &pool,
                    &correlation_id,
                    input_tokens,
                    output_tokens,
                    cost_sats,
                    stream_duration_ms,
                    success,
                    error_message.as_deref(),
                    complexity_score,
                    tier.as_deref(),
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
            }
        }
    }
    tracing::info!("DB writer task shutting down (channel closed)");
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn writer_processes_insert() {
        let pool = test_pool().await;
        let writer = DbWriter::new(pool.clone());

        writer.log_write(RequestLog {
            correlation_id: "writer-test-001".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            model: "gpt-4o".to_string(),
            provider: Some("test-provider".to_string()),
            policy: None,
            streaming: false,
            input_tokens: Some(100),
            output_tokens: Some(200),
            cost_sats: Some(10.0),
            provider_cost_sats: None,
            latency_ms: 50,
            success: true,
            error_status: None,
            error_message: None,
            complexity_score: None,
            tier: None,
        });

        // Give the writer task time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM requests WHERE correlation_id = 'writer-test-001'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn writer_processes_stream_completion_update() {
        let pool = test_pool().await;
        let writer = DbWriter::new(pool.clone());

        // Insert a row first
        writer.log_write(RequestLog {
            correlation_id: "writer-test-002".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            model: "gpt-4o".to_string(),
            provider: Some("test-provider".to_string()),
            policy: None,
            streaming: true,
            input_tokens: None,
            output_tokens: None,
            cost_sats: None,
            provider_cost_sats: None,
            latency_ms: 50,
            success: true,
            error_status: None,
            error_message: None,
            complexity_score: None,
            tier: None,
        });

        // Let insert complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Now send stream completion update
        writer.stream_completion_update(
            "writer-test-002".to_string(),
            Some(150),
            Some(300),
            Some(42.5),
            2500,
            true,
            None,
            None,
            None,
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let row: (Option<i64>, Option<i64>, Option<f64>, Option<i64>) = sqlx::query_as(
            "SELECT input_tokens, output_tokens, cost_sats, stream_duration_ms FROM requests WHERE correlation_id = 'writer-test-002'",
        )
        .bind("writer-test-002")
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.0, Some(150));
        assert_eq!(row.1, Some(300));
        assert!((row.2.unwrap() - 42.5).abs() < f64::EPSILON);
        assert_eq!(row.3, Some(2500));
    }
}
