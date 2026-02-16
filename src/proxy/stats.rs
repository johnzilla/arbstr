//! Stats endpoint types, time range resolution, and handler.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::Error;

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
