//! Request log listing endpoint types and handler.

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use super::server::AppState;
use super::stats::resolve_time_range;
use crate::error::Error;
use crate::storage;

/// Query parameters for GET /v1/requests.
#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub range: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub success: Option<bool>,
    pub streaming: Option<bool>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// Paginated response for GET /v1/requests.
#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub data: Vec<LogEntry>,
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
    pub total_pages: u32,
    pub since: String,
    pub until: String,
}

/// A single request log entry with nested sections.
#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub streaming: bool,
    pub success: bool,
    pub tokens: TokensSection,
    pub cost: CostSection,
    pub timing: TimingSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorSection>,
}

/// Token counts for a request.
#[derive(Debug, Serialize)]
pub struct TokensSection {
    pub input: Option<i64>,
    pub output: Option<i64>,
}

/// Cost information for a request.
#[derive(Debug, Serialize)]
pub struct CostSection {
    pub sats: Option<f64>,
}

/// Timing information for a request.
#[derive(Debug, Serialize)]
pub struct TimingSection {
    pub latency_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_duration_ms: Option<i64>,
}

/// Error details for a failed request.
#[derive(Debug, Serialize)]
pub struct ErrorSection {
    pub status: Option<i32>,
    pub message: Option<String>,
}

/// Validate the sort field against the allowed whitelist.
///
/// Returns the validated column name as a &'static str for safe SQL interpolation.
fn validate_sort_field(field: &str) -> Result<&'static str, Error> {
    match field {
        "timestamp" => Ok("timestamp"),
        "cost_sats" => Ok("cost_sats"),
        "latency_ms" => Ok("latency_ms"),
        _ => Err(Error::BadRequest(format!(
            "Invalid sort field '{}'. Valid options: timestamp, cost_sats, latency_ms",
            field
        ))),
    }
}

/// Validate the sort order.
///
/// Returns "ASC" or "DESC" as a &'static str for safe SQL interpolation.
fn validate_sort_order(order: &str) -> Result<&'static str, Error> {
    match order.to_lowercase().as_str() {
        "asc" => Ok("ASC"),
        "desc" => Ok("DESC"),
        _ => Err(Error::BadRequest(format!(
            "Invalid sort order '{}'. Valid options: asc, desc",
            order
        ))),
    }
}

/// Handle GET /v1/requests -- paginated request log listing.
pub async fn logs_handler(
    State(state): State<AppState>,
    Query(params): Query<LogsQuery>,
) -> Result<impl IntoResponse, Error> {
    let pool = state
        .read_db
        .as_ref()
        .ok_or_else(|| Error::Internal("Database not available".to_string()))?;

    // Resolve time range (reuses stats logic)
    let (since_dt, until_dt) = resolve_time_range(
        params.range.as_deref(),
        params.since.as_deref(),
        params.until.as_deref(),
    )?;
    let since_str = since_dt.to_rfc3339();
    let until_str = until_dt.to_rfc3339();

    tracing::debug!(
        since = %since_str,
        until = %until_str,
        model = ?params.model,
        provider = ?params.provider,
        success = ?params.success,
        streaming = ?params.streaming,
        page = ?params.page,
        per_page = ?params.per_page,
        sort = ?params.sort,
        order = ?params.order,
        "Logs query"
    );

    // Validate model filter (config check -> DB existence -> 404)
    if let Some(ref model_filter) = params.model {
        let in_config = state.config.providers.iter().any(|p| {
            p.models
                .iter()
                .any(|m| m.eq_ignore_ascii_case(model_filter))
        });
        if !in_config {
            let in_db = storage::stats::exists_in_db(pool, "model", model_filter).await?;
            if !in_db {
                return Err(Error::NotFound(format!(
                    "Model '{}' not found",
                    model_filter
                )));
            }
        }
    }

    // Validate provider filter (config check -> DB existence -> 404)
    if let Some(ref provider_filter) = params.provider {
        let in_config = state
            .config
            .providers
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(provider_filter));
        if !in_config {
            let in_db = storage::stats::exists_in_db(pool, "provider", provider_filter).await?;
            if !in_db {
                return Err(Error::NotFound(format!(
                    "Provider '{}' not found",
                    provider_filter
                )));
            }
        }
    }

    // Validate sort field (default: timestamp)
    let sort_column = match &params.sort {
        Some(field) => validate_sort_field(field)?,
        None => "timestamp",
    };

    // Validate sort order (default: DESC)
    let sort_direction = match &params.order {
        Some(order) => validate_sort_order(order)?,
        None => "DESC",
    };

    // Pagination defaults: page=1 (min 1), per_page=20 (min 1, max 100)
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);

    // Count total matching records
    let total = storage::logs::count_logs(
        pool,
        &since_str,
        &until_str,
        params.model.as_deref(),
        params.provider.as_deref(),
        params.success,
        params.streaming,
    )
    .await?;

    // Compute pagination
    let total_pages = if total == 0 {
        0
    } else {
        (total as u32).div_ceil(per_page)
    };
    let offset = (page - 1) * per_page;

    // Query the page
    let rows = storage::logs::query_logs(
        pool,
        &since_str,
        &until_str,
        params.model.as_deref(),
        params.provider.as_deref(),
        params.success,
        params.streaming,
        sort_column,
        sort_direction,
        per_page,
        offset,
    )
    .await?;

    // Map LogRow -> LogEntry
    let data: Vec<LogEntry> = rows
        .into_iter()
        .map(|row| {
            let error = if row.error_status.is_some() || row.error_message.is_some() {
                Some(ErrorSection {
                    status: row.error_status,
                    message: row.error_message,
                })
            } else {
                None
            };

            LogEntry {
                id: row.id,
                timestamp: row.timestamp,
                model: row.model,
                provider: row.provider,
                streaming: row.streaming,
                success: row.success,
                tokens: TokensSection {
                    input: row.input_tokens,
                    output: row.output_tokens,
                },
                cost: CostSection {
                    sats: row.cost_sats,
                },
                timing: TimingSection {
                    latency_ms: row.latency_ms,
                    stream_duration_ms: row.stream_duration_ms,
                },
                error,
            }
        })
        .collect();

    Ok(Json(LogsResponse {
        data,
        page,
        per_page,
        total,
        total_pages,
        since: since_dt.to_rfc3339(),
        until: until_dt.to_rfc3339(),
    }))
}
