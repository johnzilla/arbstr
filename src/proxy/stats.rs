//! Stats endpoint types, time range resolution, and handler.

use std::collections::HashSet;

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use super::server::AppState;
use crate::error::Error;
use crate::storage;

/// Query parameters for GET /v1/stats.
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    pub range: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub group_by: Option<String>,
}

/// Preset time range options.
#[derive(Debug, Clone, Copy)]
pub enum RangePreset {
    Last1h,
    Last24h,
    Last7d,
    Last30d,
}

impl RangePreset {
    /// Parse a preset string into a RangePreset.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "last_1h" => Some(Self::Last1h),
            "last_24h" => Some(Self::Last24h),
            "last_7d" => Some(Self::Last7d),
            "last_30d" => Some(Self::Last30d),
            _ => None,
        }
    }

    /// Get the duration for this preset.
    pub fn duration(&self) -> Duration {
        match self {
            Self::Last1h => Duration::hours(1),
            Self::Last24h => Duration::hours(24),
            Self::Last7d => Duration::days(7),
            Self::Last30d => Duration::days(30),
        }
    }
}

/// Resolve the time range from query parameters.
///
/// Priority:
/// 1. Explicit `since`/`until` override everything
/// 2. `range` preset applied from current UTC time
/// 3. Default: last_7d
///
/// Returns `(since, until)` as UTC datetimes.
pub fn resolve_time_range(
    range: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), Error> {
    let now = Utc::now();

    let since_dt = if let Some(s) = since {
        DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| Error::BadRequest(format!("Invalid 'since' timestamp: {}", e)))?
    } else if let Some(r) = range {
        let preset = RangePreset::parse(r).ok_or_else(|| {
            Error::BadRequest(format!(
                "Invalid range '{}'. Supported: last_1h, last_24h, last_7d, last_30d",
                r
            ))
        })?;
        now - preset.duration()
    } else {
        // Default: last 7 days
        now - RangePreset::Last7d.duration()
    };

    let until_dt = if let Some(u) = until {
        DateTime::parse_from_rfc3339(u)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| Error::BadRequest(format!("Invalid 'until' timestamp: {}", e)))?
    } else {
        now
    };

    Ok((since_dt, until_dt))
}

/// Top-level stats response.
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub since: String,
    pub until: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub empty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub counts: CountsSection,
    pub costs: CostsSection,
    pub performance: PerformanceSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<serde_json::Value>,
}

/// Request count breakdown.
#[derive(Debug, Serialize)]
pub struct CountsSection {
    pub total: i64,
    pub success: i64,
    pub error: i64,
    pub streaming: i64,
}

/// Cost and token totals.
#[derive(Debug, Serialize)]
pub struct CostsSection {
    pub total_cost_sats: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

/// Performance metrics.
#[derive(Debug, Serialize)]
pub struct PerformanceSection {
    pub avg_latency_ms: f64,
}

/// Handle GET /v1/stats -- aggregate request statistics.
pub async fn stats_handler(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Result<impl IntoResponse, Error> {
    let pool = state
        .read_db
        .as_ref()
        .ok_or_else(|| Error::Internal("Database not available".to_string()))?;

    // Resolve time range
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
        group_by = ?params.group_by,
        "Stats query"
    );

    // Validate model filter (404 for non-existent)
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

    // Validate provider filter (404 for non-existent)
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

    // Validate group_by
    if let Some(ref gb) = params.group_by {
        if gb != "model" {
            return Err(Error::BadRequest(
                "Invalid group_by value. Supported: 'model'".to_string(),
            ));
        }
    }

    // Query aggregate stats
    let row = storage::stats::query_aggregate(
        pool,
        &since_str,
        &until_str,
        params.model.as_deref(),
        params.provider.as_deref(),
    )
    .await?;

    // Build models map if group_by=model
    let models_value = if params.group_by.as_deref() == Some("model") {
        let model_rows = storage::stats::query_grouped_by_model(
            pool,
            &since_str,
            &until_str,
            params.provider.as_deref(),
        )
        .await?;

        // Collect all configured model names (deduped)
        let mut configured_models: HashSet<String> = HashSet::new();
        for p in &state.config.providers {
            for m in &p.models {
                configured_models.insert(m.clone());
            }
        }

        // Build map from SQL rows keyed by model name
        let mut models_map = serde_json::Map::new();

        // Index SQL rows by lowercase model for lookup
        let mut sql_models: std::collections::HashMap<String, &storage::stats::ModelRow> =
            std::collections::HashMap::new();
        for mr in &model_rows {
            sql_models.insert(mr.model.to_lowercase(), mr);
        }

        // Add configured models (with zeroed stats if no traffic)
        for model_name in &configured_models {
            let key = model_name.to_lowercase();
            let value = if let Some(mr) = sql_models.remove(&key) {
                model_row_to_json(mr)
            } else {
                zeroed_model_json()
            };
            models_map.insert(model_name.clone(), value);
        }

        // Add any models from SQL not in config
        for (_, mr) in sql_models {
            models_map.insert(mr.model.clone(), model_row_to_json(mr));
        }

        Some(serde_json::Value::Object(models_map))
    } else {
        None
    };

    // Determine empty state
    let (empty, message) = if row.total_requests == 0 {
        (
            Some(true),
            Some("No requests found in the specified time range".to_string()),
        )
    } else {
        (None, None)
    };

    let response = StatsResponse {
        since: since_dt.to_rfc3339(),
        until: until_dt.to_rfc3339(),
        empty,
        message,
        counts: CountsSection {
            total: row.total_requests,
            success: row.success_count,
            error: row.error_count,
            streaming: row.streaming_count,
        },
        costs: CostsSection {
            total_cost_sats: row.total_cost_sats,
            total_input_tokens: row.total_input_tokens as i64,
            total_output_tokens: row.total_output_tokens as i64,
        },
        performance: PerformanceSection {
            avg_latency_ms: row.avg_latency_ms,
        },
        models: models_value,
    };

    Ok(Json(response))
}

/// Convert a ModelRow to JSON for the models map.
fn model_row_to_json(mr: &storage::stats::ModelRow) -> serde_json::Value {
    serde_json::json!({
        "counts": {
            "total": mr.total_requests,
            "success": mr.success_count,
            "error": mr.error_count,
            "streaming": mr.streaming_count,
        },
        "costs": {
            "total_cost_sats": mr.total_cost_sats,
            "total_input_tokens": mr.total_input_tokens as i64,
            "total_output_tokens": mr.total_output_tokens as i64,
        },
        "performance": {
            "avg_latency_ms": mr.avg_latency_ms,
        }
    })
}

/// Return zeroed stats JSON for a configured model with no traffic.
fn zeroed_model_json() -> serde_json::Value {
    serde_json::json!({
        "counts": {
            "total": 0,
            "success": 0,
            "error": 0,
            "streaming": 0,
        },
        "costs": {
            "total_cost_sats": 0.0,
            "total_input_tokens": 0,
            "total_output_tokens": 0,
        },
        "performance": {
            "avg_latency_ms": 0.0,
        }
    })
}
