# Phase 11: Aggregate Stats and Filtering - Research

**Researched:** 2026-02-16
**Domain:** Read-only SQLite analytics endpoints via axum + sqlx
**Confidence:** HIGH

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions

#### Endpoint paths
- Stats endpoints live under `/v1/stats/*` alongside existing `/v1/chat/completions` and `/v1/models`
- Single endpoint at `/v1/stats` with `group_by=model` query param for per-model breakdown (not separate endpoints)
- Phase 12 request logs will live at `/v1/requests` (separate top-level path, not under /stats)
- Optional API key support -- not required (local proxy), but support an optional auth header

#### Response shape
- Nested sections in JSON response: `counts`, `costs`, `performance` groupings (not flat)
- Minimal metadata: include `since` and `until` timestamps in response (not full filter echo)
- Per-model grouped results use object keyed by model name: `{"models": {"gpt-4o": {"counts": {...}, "costs": {...}}, ...}}`
- Include all known/configured models in grouped results, even those with zero traffic in the queried window

#### Default behavior
- Default time range when no params specified: last 7 days (`last_7d`)
- Empty results return zeroed stats with `"empty": true` and a `"message"` field alongside the data
- If both `range` preset and explicit `since`/`until` provided, explicit params win (override preset)
- Presets computed from server clock in UTC at request time

#### Filter semantics
- Model and provider filters use exact match only (no prefix/partial matching)
- Matching is case-insensitive (model=GPT-4O matches stored gpt-4o)
- Single filter value only per parameter (no comma-separated or repeated params)
- Filtering by a non-existent model or provider returns 404 (helps catch typos)

### Claude's Discretion
- SQL query structure and optimization
- Exact nested field names within counts/costs/performance sections
- Error response format (should follow existing OpenAI-compatible pattern)
- Read-only connection pool implementation details

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope.

</user_constraints>

## Summary

This phase adds a single `GET /v1/stats` endpoint that computes aggregate cost and performance metrics from the existing `requests` SQLite table. The endpoint supports time range scoping (presets and explicit ISO 8601 timestamps), model/provider filtering, and per-model grouped breakdown via `group_by=model`. Zero new dependencies are required -- the existing stack (axum 0.7, sqlx 0.8, chrono 0.4, serde, serde_json) covers everything.

The core technical challenge is modest: a handful of SQL aggregate queries over a well-indexed table, wrapped in a single axum handler with query parameter extraction. The main design decisions involve the read-only connection pool separation (to prevent analytics queries from starving the proxy's write path), correct timestamp handling (the `requests.timestamp` column stores RFC 3339 text, which sorts lexicographically for text comparisons), and the case-insensitive filter matching (use `LOWER()` in SQL since the column lacks a `COLLATE NOCASE` index and the dataset is small enough that full scans on filtered results are acceptable).

The most important implementation detail is using `TOTAL()` instead of `SUM()` for nullable cost/token columns. SQLite's `SUM()` returns NULL when all values are NULL (e.g., no matching rows), while `TOTAL()` returns 0.0 -- which matches the user decision that empty results return zeroed stats rather than nulls.

**Primary recommendation:** Build a single handler function with a `StatsQuery` serde struct for query param extraction, a time range resolution layer (preset OR explicit), and two SQL query paths (aggregate vs. group-by-model), all reading from a dedicated read-only `SqlitePool` added to `AppState`.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| axum | 0.7 | HTTP server, route registration, `Query` extractor | Already used in project, `Query<T>` handles deserialization + 400 on bad params |
| sqlx | 0.8 | SQLite queries, `SqlitePool`, `SqliteConnectOptions::read_only(true)` | Already used, supports separate read-only pools natively |
| chrono | 0.4 | `Utc::now()`, `DateTime::parse_from_rfc3339()`, time arithmetic | Already used for timestamp generation in logging |
| serde | 1 | `Deserialize` for query params, `Serialize` for response | Already used throughout |
| serde_json | 1 | JSON response construction | Already used throughout |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tracing | 0.1 | Log SQL queries, filter misses, timing | Already used, follow existing patterns |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `TOTAL()` | `SUM()` + `COALESCE()` | TOTAL() is simpler -- returns 0.0 natively for empty sets |
| `LOWER()` in SQL | `COLLATE NOCASE` index | Would require migration to add collated index; LOWER() is fine for filtered aggregates on small datasets |
| `axum_extra::Query` | `axum::extract::Query` | axum_extra supports repeated params, but we only need single-value params |

**Installation:**
```bash
# No new dependencies needed. All libraries already in Cargo.toml.
```

## Architecture Patterns

### Recommended Project Structure
```
src/
├── proxy/
│   ├── handlers.rs       # Add stats_handler function
│   ├── server.rs         # Add read_db to AppState, register /v1/stats route
│   └── stats.rs          # NEW: StatsQuery, StatsResponse types, time range logic
├── storage/
│   ├── mod.rs            # Add init_read_pool(), export stats module
│   └── stats.rs          # NEW: SQL aggregate queries
└── ...
```

### Pattern 1: Query Parameter Extraction with Defaults
**What:** Use `axum::extract::Query<StatsQuery>` with `Option` fields and `#[serde(default)]` to handle optional parameters gracefully. Axum returns 400 automatically for malformed params.
**When to use:** For all GET endpoints with optional filters.
**Example:**
```rust
// Source: axum docs https://docs.rs/axum/latest/axum/extract/struct.Query.html
use axum::extract::Query;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    /// Preset range: last_1h, last_24h, last_7d, last_30d
    pub range: Option<String>,
    /// Explicit start (ISO 8601 / RFC 3339)
    pub since: Option<String>,
    /// Explicit end (ISO 8601 / RFC 3339)
    pub until: Option<String>,
    /// Filter by model name (case-insensitive exact match)
    pub model: Option<String>,
    /// Filter by provider name (case-insensitive exact match)
    pub provider: Option<String>,
    /// Group results: "model" is the only supported value
    pub group_by: Option<String>,
}

async fn stats_handler(
    State(state): State<AppState>,
    Query(params): Query<StatsQuery>,
) -> Result<impl IntoResponse, Error> {
    // ...
}
```

### Pattern 2: Time Range Resolution
**What:** Resolve the `range` preset or explicit `since`/`until` into a concrete `(DateTime<Utc>, DateTime<Utc>)` pair. Explicit params override presets. Default is `last_7d`.
**When to use:** Before building SQL queries.
**Example:**
```rust
use chrono::{DateTime, Duration, Utc};

/// Supported range presets.
pub enum RangePreset {
    Last1h,
    Last24h,
    Last7d,
    Last30d,
}

impl RangePreset {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "last_1h" => Some(Self::Last1h),
            "last_24h" => Some(Self::Last24h),
            "last_7d" => Some(Self::Last7d),
            "last_30d" => Some(Self::Last30d),
            _ => None,
        }
    }

    pub fn duration(&self) -> Duration {
        match self {
            Self::Last1h => Duration::hours(1),
            Self::Last24h => Duration::hours(24),
            Self::Last7d => Duration::days(7),
            Self::Last30d => Duration::days(30),
        }
    }
}

/// Resolve time range from query params.
/// Explicit since/until override preset. Default: last_7d.
pub fn resolve_time_range(
    range: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(DateTime<Utc>, DateTime<Utc>), Error> {
    let now = Utc::now();

    // Explicit params win over preset
    let resolved_since = match since {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map_err(|e| Error::BadRequest(format!("Invalid 'since' timestamp: {}", e)))?
            .with_timezone(&Utc),
        None => {
            let preset = range
                .and_then(RangePreset::parse)
                .unwrap_or(RangePreset::Last7d);
            now - preset.duration()
        }
    };

    let resolved_until = match until {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map_err(|e| Error::BadRequest(format!("Invalid 'until' timestamp: {}", e)))?
            .with_timezone(&Utc),
        None => now,
    };

    Ok((resolved_since, resolved_until))
}
```

### Pattern 3: Read-Only Connection Pool in AppState
**What:** Add a separate `SqlitePool` opened with `read_only(true)` for analytics queries. This prevents long-running aggregate reads from blocking the write path (fire-and-forget INSERT/UPDATE).
**When to use:** For all stats/analytics queries.
**Example:**
```rust
// In storage/mod.rs
pub async fn init_read_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
        .read_only(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(opts)
        .await?;

    Ok(pool)
}

// In server.rs -- add to AppState
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: Option<SqlitePool>,          // write pool (existing)
    pub read_db: Option<SqlitePool>,     // read-only pool (new)
}
```

### Pattern 4: SQL Aggregate Queries with TOTAL()
**What:** Use `TOTAL()` for nullable numeric columns to guarantee 0.0 instead of NULL for empty result sets. Use `COUNT(*)` for total, `AVG()` for averages, conditional `COUNT()` for success/error/streaming counts.
**When to use:** All aggregate stat computations.
**Example:**
```rust
// In storage/stats.rs
pub async fn query_aggregate_stats(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<AggregateStats, sqlx::Error> {
    // Build query dynamically based on filters
    let mut sql = String::from(
        "SELECT
            COUNT(*) as total_requests,
            TOTAL(cost_sats) as total_cost_sats,
            TOTAL(input_tokens) as total_input_tokens,
            TOTAL(output_tokens) as total_output_tokens,
            AVG(latency_ms) as avg_latency_ms,
            COUNT(CASE WHEN success = 1 THEN 1 END) as success_count,
            COUNT(CASE WHEN success = 0 THEN 1 END) as error_count,
            COUNT(CASE WHEN streaming = 1 THEN 1 END) as streaming_count
        FROM requests
        WHERE timestamp >= ? AND timestamp <= ?"
    );

    // Append filter clauses
    if model.is_some() {
        sql.push_str(" AND LOWER(model) = LOWER(?)");
    }
    if provider.is_some() {
        sql.push_str(" AND LOWER(provider) = LOWER(?)");
    }

    // ... bind and execute
}
```

### Pattern 5: Existence Check for 404 on Unknown Filters
**What:** Before running aggregate query, verify the model/provider exists in config or DB. Return 404 if not found (per user decision: helps catch typos).
**When to use:** When `model` or `provider` query param is provided.
**Example:**
```rust
// Check model exists in configured providers
if let Some(ref model_filter) = params.model {
    let known = state.config.providers.iter()
        .any(|p| p.models.iter().any(|m| m.eq_ignore_ascii_case(model_filter)));
    if !known {
        // Also check if model exists in request history
        let in_db = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM requests WHERE LOWER(model) = LOWER(?)"
        )
        .bind(model_filter)
        .fetch_one(read_pool)
        .await?;

        if in_db == 0 {
            return Err(Error::NotFound(format!("Model '{}' not found", model_filter)));
        }
    }
}
```

### Pattern 6: Response Structure with Nested Sections
**What:** Build the JSON response with `counts`, `costs`, `performance` groupings using `serde_json::json!()` macro.
**When to use:** Constructing all stats responses.
**Example:**
```rust
use serde::Serialize;

#[derive(Serialize)]
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
    /// Present only when group_by=model
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct CountsSection {
    pub total: i64,
    pub success: i64,
    pub error: i64,
    pub streaming: i64,
}

#[derive(Serialize)]
pub struct CostsSection {
    pub total_cost_sats: f64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

#[derive(Serialize)]
pub struct PerformanceSection {
    pub avg_latency_ms: f64,
}
```

### Anti-Patterns to Avoid
- **String concatenation for SQL injection:** Never interpolate user input into SQL strings. Use parameterized queries with `?` placeholders. Model/provider names come from query params and MUST be bound, not concatenated.
- **Using SUM() for nullable columns:** `SUM()` returns NULL when no rows match, requiring `COALESCE()` wrappers. `TOTAL()` returns 0.0 natively. Use `TOTAL()`.
- **Running migrations on read-only pool:** The read-only pool must NOT run `sqlx::migrate!()`. Only the write pool (existing) runs migrations. The read-only pool just connects.
- **Mixing timestamp formats:** The existing code uses `chrono::Utc::now().to_rfc3339()` which produces `+00:00` suffix (not `Z`). All timestamp comparisons must use the same format. Use `to_rfc3339()` consistently.
- **Blocking async with synchronous DB calls:** All sqlx queries are already async. Do not use `.blocking()` or spawn blocking tasks for queries.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Query param parsing | Manual URL parsing / regex | `axum::extract::Query<T>` with serde `Deserialize` | Handles missing/malformed params, returns 400 automatically |
| ISO 8601 timestamp parsing | `str::split` / regex parsing | `chrono::DateTime::parse_from_rfc3339()` | Handles timezones, validation, edge cases correctly |
| JSON response building | Manual string formatting | `serde_json::json!()` macro or derive `Serialize` | Type-safe, handles escaping, nesting |
| Connection pool management | Manual connection tracking | `sqlx::SqlitePool` | Handles connection lifecycle, health checks, retries |
| Time arithmetic | Manual seconds/days calculation | `chrono::Duration::days()`, `Duration::hours()` | Handles month boundaries, leap considerations |

**Key insight:** Every component of this phase has a well-supported library solution already in the project's dependency tree. There is zero reason to add new crates or hand-roll any infrastructure.

## Common Pitfalls

### Pitfall 1: NULL vs 0 in Aggregate Results
**What goes wrong:** `SUM(cost_sats)` returns NULL when all rows have NULL cost (e.g., failed requests) or when no rows match the filter. The JSON response then contains `null` instead of `0`.
**Why it happens:** SQL standard mandates `SUM()` returns NULL for empty sets.
**How to avoid:** Use `TOTAL()` for all nullable numeric aggregations (`cost_sats`, `input_tokens`, `output_tokens`). `TOTAL()` always returns a float, returning 0.0 for empty/all-NULL sets.
**Warning signs:** Tests with empty time windows returning `null` in JSON instead of `0`.

### Pitfall 2: Timestamp Format Mismatch in SQL Comparisons
**What goes wrong:** RFC 3339 timestamps stored in SQLite are compared as TEXT. If the format is inconsistent (some with `Z`, some with `+00:00`, some with fractional seconds), lexicographic comparison fails.
**Why it happens:** `chrono::Utc::now().to_rfc3339()` produces `2026-02-16T12:00:00.123456789+00:00` with nanosecond precision and `+00:00` suffix. `parse_from_rfc3339` accepts both `Z` and `+00:00`.
**How to avoid:** When converting user-provided timestamps for SQL comparison, normalize to the same format as stored data: `datetime.to_rfc3339()`. Consider using `to_rfc3339_opts(SecondsFormat::Millis, false)` if you want consistency, but the default works because `+00:00` sorts correctly against other `+00:00` timestamps.
**Warning signs:** Stats queries missing records near time boundaries.

### Pitfall 3: AVG() Returns NULL for Empty Sets
**What goes wrong:** `AVG(latency_ms)` returns NULL when no rows match, even though `latency_ms` is NOT NULL in the schema.
**Why it happens:** SQL `AVG()` on zero rows is NULL by definition.
**How to avoid:** Use `COALESCE(AVG(latency_ms), 0)` or handle NULL in Rust with `.unwrap_or(0.0)`.
**Warning signs:** Panic on `.unwrap()` of a NULL avg_latency value from sqlx.

### Pitfall 4: Case-Insensitive Matching Without Index
**What goes wrong:** Using `LOWER(model) = LOWER(?)` in WHERE clause prevents index usage on the `model` column.
**Why it happens:** SQLite cannot use an index when the column is wrapped in a function.
**How to avoid:** Accept the tradeoff -- for analytics queries on a local proxy's request log, table scan performance is acceptable (thousands of rows, not millions). If performance becomes an issue later, add a `COLLATE NOCASE` index via migration.
**Warning signs:** Slow queries on very large databases (unlikely for local proxy use case).

### Pitfall 5: Read-Only Pool Attempting Migrations
**What goes wrong:** Calling `sqlx::migrate!().run(&read_pool)` on a read-only pool causes a write attempt that fails with a SQLite error.
**Why it happens:** The read-only pool cannot create the `_sqlx_migrations` table.
**How to avoid:** Only run migrations on the write pool. Initialize the read pool AFTER the write pool has applied migrations.
**Warning signs:** Startup crash with "attempt to write a readonly database" error.

### Pitfall 6: Forgetting to Include Zero-Traffic Models in group_by Response
**What goes wrong:** SQL `GROUP BY model` only returns models that have rows in the time range. The user decision requires ALL configured models to appear, even with zero traffic.
**Why it happens:** SQL aggregate queries naturally exclude groups with no rows.
**How to avoid:** After running the SQL GROUP BY query, iterate over `state.config.providers` to collect all configured model names. For any model not in the SQL results, insert a zeroed stats entry.
**Warning signs:** Models disappearing from grouped response during quiet periods.

### Pitfall 7: Success Rate Division by Zero
**What goes wrong:** Computing `success_count / total_requests * 100` when `total_requests` is 0 causes division by zero.
**Why it happens:** Empty time window with no matching requests.
**How to avoid:** Return `0.0` (or omit) success_rate when total_requests is 0. Note: the response schema uses counts (success, error) rather than a computed rate, so this may not apply directly, but be careful if adding derived metrics.
**Warning signs:** Panic or NaN in JSON response.

## Code Examples

Verified patterns from existing codebase and official sources:

### Read-Only Pool Initialization
```rust
// Source: sqlx docs - SqliteConnectOptions::read_only
// https://docs.rs/sqlx/latest/sqlx/sqlite/struct.SqliteConnectOptions.html
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn init_read_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
        .read_only(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(3)  // Enough for concurrent stats queries
        .connect_with(opts)
        .await?;

    Ok(pool)
}
```

### Aggregate SQL Query with Filters
```rust
// Source: sqlx docs for dynamic query building
// https://docs.rs/sqlx/latest/sqlx/index.html
use sqlx::SqlitePool;

pub struct AggregateRow {
    pub total_requests: i64,
    pub total_cost_sats: f64,
    pub total_input_tokens: f64,   // TOTAL() returns f64
    pub total_output_tokens: f64,  // TOTAL() returns f64
    pub avg_latency_ms: Option<f64>,  // AVG() can be NULL
    pub success_count: i64,
    pub error_count: i64,
    pub streaming_count: i64,
}

pub async fn query_aggregate(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
) -> Result<AggregateRow, sqlx::Error> {
    // Use sqlx::query_as or sqlx::query with manual field extraction
    // Dynamic WHERE clause construction
    let base = "SELECT \
        COUNT(*) as total_requests, \
        TOTAL(cost_sats) as total_cost_sats, \
        TOTAL(input_tokens) as total_input_tokens, \
        TOTAL(output_tokens) as total_output_tokens, \
        COALESCE(AVG(latency_ms), 0) as avg_latency_ms, \
        COUNT(CASE WHEN success = 1 THEN 1 END) as success_count, \
        COUNT(CASE WHEN success = 0 THEN 1 END) as error_count, \
        COUNT(CASE WHEN streaming = 1 THEN 1 END) as streaming_count \
        FROM requests WHERE timestamp >= ? AND timestamp <= ?";

    // Build dynamic query with optional filters
    let mut query_str = base.to_string();
    if model.is_some() {
        query_str.push_str(" AND LOWER(model) = LOWER(?)");
    }
    if provider.is_some() {
        query_str.push_str(" AND LOWER(provider) = LOWER(?)");
    }

    let mut query = sqlx::query_as::<_, AggregateRow>(&query_str)
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
```

### Per-Model Grouped Query
```rust
// Group-by query returns one row per model
pub struct ModelRow {
    pub model: String,
    pub total_requests: i64,
    pub total_cost_sats: f64,
    pub total_input_tokens: f64,
    pub total_output_tokens: f64,
    pub avg_latency_ms: Option<f64>,
    pub success_count: i64,
    pub error_count: i64,
    pub streaming_count: i64,
}

pub async fn query_grouped_by_model(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    provider: Option<&str>,
) -> Result<Vec<ModelRow>, sqlx::Error> {
    let mut query_str = "SELECT model, \
        COUNT(*) as total_requests, \
        TOTAL(cost_sats) as total_cost_sats, \
        TOTAL(input_tokens) as total_input_tokens, \
        TOTAL(output_tokens) as total_output_tokens, \
        COALESCE(AVG(latency_ms), 0) as avg_latency_ms, \
        COUNT(CASE WHEN success = 1 THEN 1 END) as success_count, \
        COUNT(CASE WHEN success = 0 THEN 1 END) as error_count, \
        COUNT(CASE WHEN streaming = 1 THEN 1 END) as streaming_count \
        FROM requests WHERE timestamp >= ? AND timestamp <= ?"
        .to_string();

    if provider.is_some() {
        query_str.push_str(" AND LOWER(provider) = LOWER(?)");
    }
    query_str.push_str(" GROUP BY model");

    let mut query = sqlx::query_as::<_, ModelRow>(&query_str)
        .bind(since)
        .bind(until);

    if let Some(p) = provider {
        query = query.bind(p);
    }

    query.fetch_all(pool).await
}
```

### Filling Zero-Traffic Models from Config
```rust
// After SQL GROUP BY, ensure all configured models appear
use std::collections::HashMap;
use serde_json::Value;

fn build_models_response(
    sql_rows: Vec<ModelRow>,
    configured_models: &[String],
) -> serde_json::Map<String, Value> {
    let mut models_map = serde_json::Map::new();

    // Index SQL results by lowercase model name
    let mut by_model: HashMap<String, &ModelRow> = HashMap::new();
    for row in &sql_rows {
        by_model.insert(row.model.to_lowercase(), row);
    }

    // Add all configured models (including zero-traffic ones)
    for model_name in configured_models {
        let key = model_name.clone();
        let entry = match by_model.get(&model_name.to_lowercase()) {
            Some(row) => serde_json::json!({
                "counts": {
                    "total": row.total_requests,
                    "success": row.success_count,
                    "error": row.error_count,
                    "streaming": row.streaming_count,
                },
                "costs": {
                    "total_cost_sats": row.total_cost_sats,
                    "total_input_tokens": row.total_input_tokens as i64,
                    "total_output_tokens": row.total_output_tokens as i64,
                },
                "performance": {
                    "avg_latency_ms": row.avg_latency_ms.unwrap_or(0.0),
                },
            }),
            None => serde_json::json!({
                "counts": { "total": 0, "success": 0, "error": 0, "streaming": 0 },
                "costs": { "total_cost_sats": 0.0, "total_input_tokens": 0, "total_output_tokens": 0 },
                "performance": { "avg_latency_ms": 0.0 },
            }),
        };
        models_map.insert(key, entry);
    }

    // Also include models found in DB but not in current config
    for row in &sql_rows {
        if !models_map.contains_key(&row.model) {
            models_map.insert(row.model.clone(), serde_json::json!({
                "counts": {
                    "total": row.total_requests,
                    "success": row.success_count,
                    "error": row.error_count,
                    "streaming": row.streaming_count,
                },
                "costs": {
                    "total_cost_sats": row.total_cost_sats,
                    "total_input_tokens": row.total_input_tokens as i64,
                    "total_output_tokens": row.total_output_tokens as i64,
                },
                "performance": {
                    "avg_latency_ms": row.avg_latency_ms.unwrap_or(0.0),
                },
            }));
        }
    }

    models_map
}
```

### Error Type Extension for NotFound
```rust
// Add to error.rs -- new variant for 404 responses
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // ... existing variants ...

    #[error("Not found: {0}")]
    NotFound(String),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            // ... existing arms ...
            Error::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
        };

        let body = serde_json::json!({
            "error": {
                "message": message,
                "type": "arbstr_error",
                "code": status.as_u16()
            }
        });

        (status, axum::Json(body)).into_response()
    }
}
```

### Route Registration
```rust
// In server.rs -- add to create_router
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/models", get(handlers::list_models))
        .route("/v1/stats", get(handlers::stats))  // NEW
        .route("/health", get(handlers::health))
        .route("/providers", get(handlers::list_providers))
        .with_state(state)
        // ... existing middleware ...
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `SUM()` for aggregates | `TOTAL()` for nullable columns | SQLite built-in (always available) | Avoids NULL in empty result sets |
| Single SQLite pool | Separate read/write pools with WAL | sqlx 0.7+ (pool options) | Prevents analytics from starving proxy writes |
| Manual query param parsing | `axum::extract::Query<T>` | axum 0.6+ | Type-safe, auto-400 on bad input |

**Deprecated/outdated:**
- None relevant -- all libraries in use are current stable versions.

## Open Questions

1. **Timestamp precision in SQL comparisons**
   - What we know: `chrono::Utc::now().to_rfc3339()` includes nanosecond fractional seconds (e.g., `2026-02-16T12:00:00.123456789+00:00`). SQLite text comparison of these strings is lexicographic, which preserves chronological ordering when format is consistent.
   - What's unclear: Whether user-provided `since`/`until` params will include fractional seconds, and whether edge-case boundary comparisons (inclusive vs exclusive) need special handling.
   - Recommendation: Use `>=` for `since` and `<=` for `until` (inclusive range). Normalize user timestamps through `parse_from_rfc3339` then `to_rfc3339()` before passing to SQL. This ensures consistent format.

2. **Dynamic query building with sqlx**
   - What we know: sqlx supports `query()` and `query_as()` with string-based queries and positional `?` binds. Dynamic WHERE clauses require string concatenation of the SQL (not the values).
   - What's unclear: Whether `sqlx::QueryBuilder` is a better fit for dynamic filter construction.
   - Recommendation: Use simple string concatenation for the SQL template (safe because filter clause additions are hardcoded strings, not user input). Bind values with `?` placeholders. The query has at most 4 dynamic clauses -- `QueryBuilder` is overkill.

3. **Should non-existent provider also check DB history?**
   - What we know: The 404 check for unknown model/provider is meant to catch typos. Configured models are in `state.config.providers`.
   - What's unclear: Should a model that exists in DB history (from a previously configured provider) but is no longer in config also be queryable?
   - Recommendation: Check config first, then fall back to DB history. A model that has been used historically should be queryable even if no longer configured.

## Sources

### Primary (HIGH confidence)
- [axum::extract::Query docs](https://docs.rs/axum/latest/axum/extract/struct.Query.html) - Query parameter extraction, serde Deserialize, 400 on parse failure
- [sqlx::sqlite::SqliteConnectOptions](https://docs.rs/sqlx/latest/sqlx/sqlite/struct.SqliteConnectOptions.html) - `read_only(true)`, `journal_mode()`, pool configuration
- [SQLite Built-in Aggregate Functions](https://sqlite.org/lang_aggfunc.html) - `TOTAL()` vs `SUM()` behavior, NULL handling, return types
- [chrono::DateTime docs](https://docs.rs/chrono/latest/chrono/struct.DateTime.html) - `parse_from_rfc3339()`, `to_rfc3339()`, timezone handling

### Secondary (MEDIUM confidence)
- [SQLite Sum() vs Total(): What's the Difference?](https://database.guide/sqlite-sum-vs-total-whats-the-difference/) - Practical comparison with examples
- [Rust & SQLite - Storing Time - Text vs Integer](https://rust10x.com/post/sqlite-time-text-vs-integer) - Timestamp format tradeoffs
- [chrono issue #157 - Z vs +00:00](https://github.com/chronotope/chrono/issues/157) - `to_rfc3339()` uses `+00:00`, `to_rfc3339_opts` can use `Z`

### Tertiary (LOW confidence)
- None -- all findings verified with primary or secondary sources.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - zero new dependencies, all libraries already in use and verified via Cargo.toml
- Architecture: HIGH - patterns follow existing codebase conventions (AppState, handlers, storage modules), sqlx read_only verified in official docs
- Pitfalls: HIGH - TOTAL() vs SUM() verified in SQLite docs, timestamp format verified by examining existing code

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable domain, no fast-moving dependencies)
