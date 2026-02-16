//! Paginated log query functions for the request listing endpoint.

use sqlx::SqlitePool;

/// A single request log row from the database.
#[derive(Debug, sqlx::FromRow)]
pub struct LogRow {
    pub id: i64,
    pub timestamp: String,
    pub model: String,
    pub provider: Option<String>,
    pub streaming: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_sats: Option<f64>,
    pub latency_ms: i64,
    pub stream_duration_ms: Option<i64>,
    pub success: bool,
    pub error_status: Option<i32>,
    pub error_message: Option<String>,
}

/// Count request logs matching the given filters.
///
/// Builds a dynamic WHERE clause with time range and optional model, provider,
/// success, and streaming filters. All string comparisons are case-insensitive.
pub async fn count_logs(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
    success: Option<bool>,
    streaming: Option<bool>,
) -> Result<i64, sqlx::Error> {
    let mut sql = String::from("SELECT COUNT(*) FROM requests WHERE timestamp >= ? AND timestamp <= ?");

    if model.is_some() {
        sql.push_str(" AND LOWER(model) = LOWER(?)");
    }
    if provider.is_some() {
        sql.push_str(" AND LOWER(provider) = LOWER(?)");
    }
    if success.is_some() {
        sql.push_str(" AND success = ?");
    }
    if streaming.is_some() {
        sql.push_str(" AND streaming = ?");
    }

    let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(since).bind(until);

    if let Some(m) = model {
        query = query.bind(m);
    }
    if let Some(p) = provider {
        query = query.bind(p);
    }
    if let Some(s) = success {
        query = query.bind(s);
    }
    if let Some(st) = streaming {
        query = query.bind(st);
    }

    query.fetch_one(pool).await
}

/// Query request logs with filtering, sorting, and pagination.
///
/// Builds a dynamic WHERE clause matching `count_logs`, then appends ORDER BY
/// and LIMIT/OFFSET. The `sort_column` and `sort_direction` parameters are
/// pre-validated &'static str values from the handler's whitelist.
pub async fn query_logs(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
    success: Option<bool>,
    streaming: Option<bool>,
    sort_column: &str,
    sort_direction: &str,
    limit: u32,
    offset: u32,
) -> Result<Vec<LogRow>, sqlx::Error> {
    let mut sql = String::from(
        "SELECT id, timestamp, model, provider, streaming, input_tokens, output_tokens, \
         cost_sats, latency_ms, stream_duration_ms, success, error_status, error_message \
         FROM requests WHERE timestamp >= ? AND timestamp <= ?",
    );

    if model.is_some() {
        sql.push_str(" AND LOWER(model) = LOWER(?)");
    }
    if provider.is_some() {
        sql.push_str(" AND LOWER(provider) = LOWER(?)");
    }
    if success.is_some() {
        sql.push_str(" AND success = ?");
    }
    if streaming.is_some() {
        sql.push_str(" AND streaming = ?");
    }

    // sort_column and sort_direction are validated &'static str -- safe to interpolate
    sql.push_str(&format!(" ORDER BY {} {}", sort_column, sort_direction));
    sql.push_str(" LIMIT ? OFFSET ?");

    let mut query = sqlx::query_as::<_, LogRow>(&sql).bind(since).bind(until);

    if let Some(m) = model {
        query = query.bind(m);
    }
    if let Some(p) = provider {
        query = query.bind(p);
    }
    if let Some(s) = success {
        query = query.bind(s);
    }
    if let Some(st) = streaming {
        query = query.bind(st);
    }

    query = query.bind(limit as i64).bind(offset as i64);

    query.fetch_all(pool).await
}
