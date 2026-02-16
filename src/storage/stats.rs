//! Aggregate statistics queries for the stats endpoint.

use sqlx::SqlitePool;

/// Aggregate statistics for a time range.
#[derive(sqlx::FromRow)]
pub struct AggregateRow {
    pub total_requests: i64,
    pub total_cost_sats: f64,
    pub total_input_tokens: f64,
    pub total_output_tokens: f64,
    pub avg_latency_ms: f64,
    pub success_count: i64,
    pub error_count: i64,
    pub streaming_count: i64,
}

/// Per-model statistics for a time range.
#[derive(sqlx::FromRow)]
pub struct ModelRow {
    pub model: String,
    pub total_requests: i64,
    pub total_cost_sats: f64,
    pub total_input_tokens: f64,
    pub total_output_tokens: f64,
    pub avg_latency_ms: f64,
    pub success_count: i64,
    pub error_count: i64,
    pub streaming_count: i64,
}

/// Query aggregate statistics for a time range with optional model/provider filters.
///
/// Uses `TOTAL()` for nullable numeric columns (returns 0.0 instead of NULL)
/// and `COALESCE(AVG(), 0)` for latency to ensure non-null results.
pub async fn query_aggregate(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<AggregateRow, sqlx::Error> {
    let mut sql = String::from(
        "SELECT \
         COUNT(*) as total_requests, \
         TOTAL(cost_sats) as total_cost_sats, \
         TOTAL(input_tokens) as total_input_tokens, \
         TOTAL(output_tokens) as total_output_tokens, \
         COALESCE(AVG(latency_ms), 0) as avg_latency_ms, \
         COUNT(CASE WHEN success = 1 THEN 1 END) as success_count, \
         COUNT(CASE WHEN success = 0 THEN 1 END) as error_count, \
         COUNT(CASE WHEN streaming = 1 THEN 1 END) as streaming_count \
         FROM requests WHERE timestamp >= ? AND timestamp <= ?",
    );

    if model.is_some() {
        sql.push_str(" AND LOWER(model) = LOWER(?)");
    }
    if provider.is_some() {
        sql.push_str(" AND LOWER(provider) = LOWER(?)");
    }

    let mut query = sqlx::query_as::<_, AggregateRow>(&sql)
        .bind(since)
        .bind(until);

    if let Some(m) = model {
        query = query.bind(m);
    }
    if let Some(p) = provider {
        query = query.bind(p);
    }

    query.fetch_one(pool).await
}

/// Query per-model statistics for a time range with optional provider filter.
///
/// Returns one row per model with aggregate stats, grouped by model name.
pub async fn query_grouped_by_model(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    provider: Option<&str>,
) -> Result<Vec<ModelRow>, sqlx::Error> {
    let mut sql = String::from(
        "SELECT \
         model, \
         COUNT(*) as total_requests, \
         TOTAL(cost_sats) as total_cost_sats, \
         TOTAL(input_tokens) as total_input_tokens, \
         TOTAL(output_tokens) as total_output_tokens, \
         COALESCE(AVG(latency_ms), 0) as avg_latency_ms, \
         COUNT(CASE WHEN success = 1 THEN 1 END) as success_count, \
         COUNT(CASE WHEN success = 0 THEN 1 END) as error_count, \
         COUNT(CASE WHEN streaming = 1 THEN 1 END) as streaming_count \
         FROM requests WHERE timestamp >= ? AND timestamp <= ?",
    );

    if provider.is_some() {
        sql.push_str(" AND LOWER(provider) = LOWER(?)");
    }

    sql.push_str(" GROUP BY model");

    let mut query = sqlx::query_as::<_, ModelRow>(&sql)
        .bind(since)
        .bind(until);

    if let Some(p) = provider {
        query = query.bind(p);
    }

    query.fetch_all(pool).await
}

/// Check whether a value exists in the requests table for a given column.
///
/// Column name is whitelisted to "model" or "provider" to prevent SQL injection.
/// Returns true if at least one row matches (case-insensitive).
pub async fn exists_in_db(
    pool: &SqlitePool,
    column: &str,
    value: &str,
) -> Result<bool, sqlx::Error> {
    let sql = match column {
        "model" => "SELECT COUNT(*) as cnt FROM requests WHERE LOWER(model) = LOWER(?)",
        "provider" => "SELECT COUNT(*) as cnt FROM requests WHERE LOWER(provider) = LOWER(?)",
        _ => return Ok(false),
    };

    let (count,): (i64,) = sqlx::query_as(sql).bind(value).fetch_one(pool).await?;

    Ok(count > 0)
}
