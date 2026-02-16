# Architecture Patterns: Cost Querying API Endpoints

**Domain:** Read-only analytics/stats endpoints in existing Rust/axum/sqlx proxy
**Researched:** 2026-02-16
**Overall confidence:** HIGH (patterns verified against existing codebase, axum Query extractor well-documented, sqlx aggregate queries are standard SQL)

## Current Architecture (Baseline)

### Existing Component Map

```
src/
  proxy/
    server.rs     -- AppState { router, http_client, config, db: Option<SqlitePool> }
                     create_router() builds axum::Router with routes + state
    handlers.rs   -- chat_completions, list_models, health, list_providers (all pub)
    mod.rs        -- declares modules, re-exports public items
  storage/
    mod.rs        -- init_pool(), re-exports from logging
    logging.rs    -- RequestLog struct, insert/update functions (write-only)
  router/
    selector.rs   -- Router, SelectedProvider, actual_cost_sats
  error.rs        -- Error enum with IntoResponse for OpenAI-compatible errors
  config.rs       -- Config, ProviderConfig, PolicyRule
  lib.rs          -- pub mod declarations
```

### How Handlers Access the Database Today

Every handler follows the same pattern:

```rust
pub async fn chat_completions(
    State(state): State<AppState>,    // axum extracts AppState from shared state
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Response, Error> {
    // ...
    if let Some(pool) = &state.db {
        spawn_log_write(pool, log_entry);   // fire-and-forget write
    }
}
```

Key observations:
- `state.db` is `Option<SqlitePool>` -- database may not be initialized
- All current database access is **write-only** (INSERT, UPDATE)
- No read queries exist anywhere in the codebase today
- The `Error::Database` variant already exists for sqlx errors
- SQLite pool uses WAL mode with max 5 connections -- reads and writes do not block each other in WAL mode

### Existing Route Registration

```rust
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/models", get(handlers::list_models))
        .route("/health", get(handlers::health))
        .route("/providers", get(handlers::list_providers))
        .with_state(state)
        .layer(TraceLayer::new_for_http().make_span_with(/* ... */))
        .layer(middleware::from_fn(inject_request_id))
}
```

### Database Schema (requests table)

```sql
CREATE TABLE requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    correlation_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,          -- RFC3339 string, indexed
    model TEXT NOT NULL,
    provider TEXT,
    policy TEXT,
    streaming BOOLEAN NOT NULL DEFAULT FALSE,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_sats REAL,
    provider_cost_sats REAL,
    latency_ms INTEGER NOT NULL,
    stream_duration_ms INTEGER,
    success BOOLEAN NOT NULL,
    error_status INTEGER,
    error_message TEXT
);

CREATE INDEX idx_requests_correlation_id ON requests(correlation_id);
CREATE INDEX idx_requests_timestamp ON requests(timestamp);
```

The `timestamp` column stores RFC3339 strings. SQLite comparison operators work correctly on ISO 8601 / RFC3339 strings for range queries (`WHERE timestamp >= ? AND timestamp < ?`) because lexicographic ordering matches chronological ordering for these formats.

---

## Recommended Architecture

### Where New Code Lives

**Decision: Create a new `src/storage/stats.rs` module alongside `logging.rs`.**

Rationale:
- Query functions need `&SqlitePool` -- they belong in `storage/`, the data access layer
- `logging.rs` is exclusively write operations (INSERT/UPDATE). Mixing reads muddies its purpose
- A `stats.rs` file keeps read concerns isolated and testable independently
- The `storage/mod.rs` already serves as the namespace root and can re-export query types

**Do NOT create a new top-level `src/stats/` module.** The data access pattern (SqlitePool in, typed results out) is the same as `logging.rs`. The storage module is the right home.

**Handler functions go in `src/proxy/handlers.rs`** -- the same file as all other handlers. The stats handlers will be simple and short (extract query params, call storage function, return JSON). There is no reason to split into a separate `stats_handlers.rs` until handlers.rs becomes unwieldy, which it will not with 3-5 new endpoints of ~15 lines each.

### File Change Summary

| File | Change | What |
|------|--------|------|
| `src/storage/stats.rs` | **NEW** | Query functions: `get_summary`, `get_cost_by_model`, `get_cost_by_provider`, `get_recent_requests` |
| `src/storage/mod.rs` | MODIFY | Add `pub mod stats;` and re-export key types |
| `src/proxy/handlers.rs` | MODIFY | Add stats handler functions + query param structs |
| `src/proxy/server.rs` | MODIFY | Register new routes in `create_router()` |
| `src/proxy/mod.rs` | MODIFY | Nothing visible (handlers are already `mod handlers`) |

No changes to: `error.rs` (Database variant exists), `config.rs`, `router/`, `lib.rs`.

### Component Boundaries

```
┌─────────────────────────────────────────────────────┐
│  proxy/handlers.rs                                  │
│                                                     │
│  stats_summary(State, Query<TimeRange>)             │
│    -> calls storage::stats::get_summary(&pool, ...) │
│    -> returns Json<SummaryResponse>                 │
│                                                     │
│  stats_by_model(State, Query<TimeRange>)            │
│    -> calls storage::stats::get_cost_by_model(...)  │
│    -> returns Json<ModelBreakdown>                  │
│                                                     │
│  stats_by_provider(State, Query<TimeRange>)         │
│    -> calls storage::stats::get_cost_by_provider()  │
│    -> returns Json<ProviderBreakdown>               │
│                                                     │
│  stats_recent(State, Query<RecentParams>)           │
│    -> calls storage::stats::get_recent_requests()   │
│    -> returns Json<RecentRequests>                  │
│                                                     │
└─────────────────┬───────────────────────────────────┘
                  │ calls
                  ▼
┌─────────────────────────────────────────────────────┐
│  storage/stats.rs                                   │
│                                                     │
│  Response types: SummaryRow, ModelCostRow, etc.     │
│  Query functions: async fn -> Result<T, sqlx::Error>│
│  Pure data access, no HTTP concerns                 │
│                                                     │
└─────────────────┬───────────────────────────────────┘
                  │ sqlx queries
                  ▼
┌─────────────────────────────────────────────────────┐
│  SQLite (requests table, WAL mode)                  │
└─────────────────────────────────────────────────────┘
```

### Data Flow for a Stats Request

```
GET /v1/arbstr/stats/summary?since=2026-02-01T00:00:00Z&until=2026-02-16T00:00:00Z

1. axum extracts State<AppState> and Query<TimeRange>
2. Handler checks state.db is Some (returns 503 if None)
3. Handler calls storage::stats::get_summary(&pool, since, until)
4. stats.rs executes SQL aggregate query
5. sqlx returns typed result (SummaryRow or Vec<ModelCostRow>)
6. Handler wraps in Json() and returns
```

---

## Endpoint Design

### URL Structure: `/v1/arbstr/stats/*`

Use the `/v1/arbstr/` prefix for arbstr-specific extensions. This is consistent with the existing arbstr extension pattern (the project already adds `arbstr_provider` to response bodies and uses `x-arbstr-*` headers). The `/v1/` prefix signals API versioning. The `stats/` segment groups all analytics endpoints.

Endpoints:

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/v1/arbstr/stats/summary` | Overall totals: request count, tokens, cost, latency |
| GET | `/v1/arbstr/stats/models` | Cost and usage broken down by model |
| GET | `/v1/arbstr/stats/providers` | Cost and usage broken down by provider |
| GET | `/v1/arbstr/stats/requests` | Recent individual request log entries |

### Route Registration Pattern

Use `axum::Router::nest` to group stats routes under a prefix, then merge into the main router. This keeps `create_router()` clean:

```rust
pub fn create_router(state: AppState) -> Router {
    let stats_routes = Router::new()
        .route("/summary", get(handlers::stats_summary))
        .route("/models", get(handlers::stats_by_model))
        .route("/providers", get(handlers::stats_by_provider))
        .route("/requests", get(handlers::stats_recent));

    Router::new()
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/models", get(handlers::list_models))
        .route("/health", get(handlers::health))
        .route("/providers", get(handlers::list_providers))
        .nest("/v1/arbstr/stats", stats_routes)
        .with_state(state)
        .layer(/* ... */)
}
```

Note: `nest()` strips the prefix from the nested router's perspective, so routes inside are registered as `/summary`, `/models`, etc. The full path is `/v1/arbstr/stats/summary`. All nested routes share the same `AppState` and middleware layers applied after nesting.

---

## Query Parameter Design

### axum Query Extractor Pattern

The `axum::extract::Query<T>` extractor deserializes URL query parameters into a struct `T` that implements `serde::Deserialize`. For optional parameters, use `Option<T>` fields with `#[serde(default)]`:

```rust
use axum::extract::Query;
use serde::Deserialize;

/// Query parameters for time-ranged stats endpoints.
#[derive(Debug, Deserialize)]
pub struct TimeRangeParams {
    /// Start of time range (RFC3339). Defaults to 24 hours ago if omitted.
    pub since: Option<String>,
    /// End of time range (RFC3339). Defaults to now if omitted.
    pub until: Option<String>,
}

/// Query parameters for the recent requests endpoint.
#[derive(Debug, Deserialize)]
pub struct RecentParams {
    /// Maximum number of results. Defaults to 50, max 500.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Start of time range (RFC3339). Optional.
    pub since: Option<String>,
    /// End of time range (RFC3339). Optional.
    pub until: Option<String>,
}

fn default_limit() -> u32 { 50 }
```

**Why String instead of chrono::DateTime for timestamps:** The `timestamp` column in SQLite stores RFC3339 strings. Keeping query params as strings avoids a parse-format round-trip. The handler validates the format, and the SQL uses string comparison directly. If a client sends a malformed timestamp, the handler returns a 400 error before reaching the database.

**How axum handles missing query strings:** When no query string is present at all, axum passes an empty string `""` to serde. With all-optional fields (`Option<T>`) this deserializes successfully with all fields as `None`. No need for `axum_extra::OptionalQuery`.

### Handler Pattern

```rust
pub async fn stats_summary(
    State(state): State<AppState>,
    Query(params): Query<TimeRangeParams>,
) -> Result<impl IntoResponse, Error> {
    let pool = state.db.as_ref().ok_or_else(|| {
        Error::Internal("Database not available".to_string())
    })?;

    let (since, until) = resolve_time_range(params.since, params.until)?;

    let summary = crate::storage::stats::get_summary(pool, &since, &until)
        .await
        .map_err(Error::Database)?;

    Ok(Json(summary))
}
```

Key design choices:
- Return `Error::Internal` for missing database (503 semantics via the Error enum, or add a dedicated variant)
- `resolve_time_range()` is a shared helper that applies defaults (24h ago / now) and validates RFC3339 format
- The `?` operator with `Error::Database` converts `sqlx::Error` naturally (the `From` impl exists)

---

## Storage Query Layer Design

### stats.rs Structure

```rust
//! Read-only analytics queries against the requests table.

use serde::Serialize;
use sqlx::SqlitePool;

/// Overall summary statistics for a time range.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub total_requests: i64,
    pub successful_requests: i64,
    pub failed_requests: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_sats: f64,
    pub avg_latency_ms: f64,
    pub since: String,
    pub until: String,
}

/// Cost and usage breakdown for a single model.
#[derive(Debug, Serialize)]
pub struct ModelStats {
    pub model: String,
    pub request_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_sats: f64,
    pub avg_latency_ms: f64,
}

/// Cost and usage breakdown for a single provider.
#[derive(Debug, Serialize)]
pub struct ProviderStats {
    pub provider: String,
    pub request_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_sats: f64,
    pub avg_latency_ms: f64,
    pub success_rate: f64,
}

/// A single request log entry for the recent requests endpoint.
#[derive(Debug, Serialize)]
pub struct RequestEntry {
    pub correlation_id: String,
    pub timestamp: String,
    pub model: String,
    pub provider: Option<String>,
    pub policy: Option<String>,
    pub streaming: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cost_sats: Option<f64>,
    pub latency_ms: i64,
    pub success: bool,
    pub error_message: Option<String>,
}
```

### Query Functions

Each function takes `&SqlitePool` and time range bounds, returns `Result<T, sqlx::Error>`:

```rust
pub async fn get_summary(
    pool: &SqlitePool,
    since: &str,
    until: &str,
) -> Result<Summary, sqlx::Error> {
    // Use sqlx::query_as or manual query + FromRow
    let row: (i64, i64, i64, Option<i64>, Option<i64>, Option<f64>, Option<f64>) =
        sqlx::query_as(
            "SELECT
                COUNT(*) as total,
                SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END),
                SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END),
                SUM(input_tokens),
                SUM(output_tokens),
                SUM(cost_sats),
                AVG(latency_ms)
            FROM requests
            WHERE timestamp >= ? AND timestamp < ?"
        )
        .bind(since)
        .bind(until)
        .fetch_one(pool)
        .await?;

    Ok(Summary {
        total_requests: row.0,
        successful_requests: row.1,
        failed_requests: row.2,
        total_input_tokens: row.3.unwrap_or(0),
        total_output_tokens: row.4.unwrap_or(0),
        total_cost_sats: row.5.unwrap_or(0.0),
        avg_latency_ms: row.6.unwrap_or(0.0),
        since: since.to_string(),
        until: until.to_string(),
    })
}
```

**Why `query_as` with tuples instead of `FromRow` derive:** The aggregate results (SUM, AVG, COUNT) do not directly map to a table row. Using tuple extraction with `query_as` is the simplest approach for aggregate queries. The `FromRow` derive macro is better suited for selecting full rows (used in `get_recent_requests`).

**Why `Option` for SUM/AVG columns:** SQLite aggregate functions return NULL when no rows match the filter. Using `Option<i64>` / `Option<f64>` and `.unwrap_or(0)` handles this cleanly.

### Using FromRow for Recent Requests

For the recent requests endpoint, which selects individual rows rather than aggregates, use `sqlx::FromRow`:

```rust
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RequestEntry {
    pub correlation_id: String,
    pub timestamp: String,
    pub model: String,
    pub provider: Option<String>,
    // ... etc
}

pub async fn get_recent_requests(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    limit: u32,
) -> Result<Vec<RequestEntry>, sqlx::Error> {
    sqlx::query_as::<_, RequestEntry>(
        "SELECT correlation_id, timestamp, model, provider, policy,
                streaming, input_tokens, output_tokens, cost_sats,
                latency_ms, success, error_message
         FROM requests
         WHERE timestamp >= ? AND timestamp < ?
         ORDER BY timestamp DESC
         LIMIT ?"
    )
    .bind(since)
    .bind(until)
    .bind(limit)
    .fetch_all(pool)
    .await
}
```

---

## Patterns to Follow

### Pattern 1: Graceful Database Absence

The existing codebase treats `state.db` as optional. Stats endpoints should follow the same pattern but with different semantics:

- **Write endpoints (chat_completions):** Skip logging silently when db is None. The primary function (proxying) still works.
- **Read endpoints (stats):** Return an error when db is None. Stats are the primary function; they cannot work without a database.

Return a 503 Service Unavailable (not 500) because this is a transient/configuration issue, not a bug:

```rust
let pool = state.db.as_ref().ok_or_else(|| {
    Error::Internal("Database not configured; stats unavailable".to_string())
})?;
```

Consider adding an `Error::ServiceUnavailable(String)` variant that maps to HTTP 503, or handle the mapping in the handler with a manual Response. The existing `Error::Internal` maps to 500, which is slightly wrong semantically but acceptable for v1.

### Pattern 2: Time Range Resolution Helper

A shared helper function to apply defaults and validate timestamps, used by all stats handlers:

```rust
/// Resolve optional time range parameters into concrete RFC3339 bounds.
///
/// Defaults: since = 24 hours ago, until = now.
fn resolve_time_range(
    since: Option<String>,
    until: Option<String>,
) -> Result<(String, String), Error> {
    let now = chrono::Utc::now();
    let default_since = (now - chrono::Duration::hours(24)).to_rfc3339();
    let default_until = now.to_rfc3339();

    let since = since.unwrap_or(default_since);
    let until = until.unwrap_or(default_until);

    // Validate RFC3339 format by attempting parse
    chrono::DateTime::parse_from_rfc3339(&since)
        .map_err(|_| Error::BadRequest(format!("Invalid 'since' timestamp: {}", since)))?;
    chrono::DateTime::parse_from_rfc3339(&until)
        .map_err(|_| Error::BadRequest(format!("Invalid 'until' timestamp: {}", until)))?;

    Ok((since, until))
}
```

This gives clear 400 errors for malformed input, uses chrono (already in Cargo.toml), and keeps the validated string as the query parameter (no DateTime-to-string round-trip needed).

### Pattern 3: Consistent JSON Response Wrapping

Follow the existing pattern from `list_models` and `list_providers`: top-level JSON object with a descriptive key, not a bare array:

```rust
// Good: consistent with existing endpoints
Json(serde_json::json!({
    "summary": summary,
    "since": since,
    "until": until,
}))

// Good: for array results
Json(serde_json::json!({
    "models": model_stats,
    "since": since,
    "until": until,
}))

// Bad: bare array at top level
Json(model_stats)
```

---

## Anti-Patterns to Avoid

### Anti-Pattern 1: Blocking Queries on the Tokio Runtime

**What:** Using synchronous SQLite calls or CPU-intensive post-processing on query results in an async handler.
**Why bad:** Blocks the Tokio executor thread, starving other tasks (including the proxying pipeline).
**Instead:** Use sqlx's async API exclusively (which this architecture does). If post-processing is needed (e.g., percentile calculations), keep it cheap or use `tokio::task::spawn_blocking`.

### Anti-Pattern 2: Query Logic in Handlers

**What:** Writing SQL queries directly inside handler functions.
**Why bad:** Handlers become untestable without a live database. SQL gets scattered across the proxy layer.
**Instead:** All SQL lives in `storage/stats.rs`. Handlers call typed functions. Storage functions are independently testable with in-memory SQLite (existing test pattern in `logging.rs`).

### Anti-Pattern 3: Creating New Database Connections per Request

**What:** Opening a new SQLite connection for each stats query.
**Why bad:** Connection overhead, no connection reuse, no WAL benefit.
**Instead:** Use the shared `SqlitePool` from `AppState.db`. sqlx manages connection pooling automatically. The existing pool has `max_connections(5)` which is adequate for a local proxy; reads in WAL mode do not block writes.

### Anti-Pattern 4: Unparameterized SQL with String Formatting

**What:** Building SQL queries with `format!("... WHERE timestamp >= '{}'", since)`.
**Why bad:** SQL injection risk, no query plan caching.
**Instead:** Always use `sqlx::query().bind()` with parameterized queries (existing codebase pattern).

---

## Index Considerations

The existing `idx_requests_timestamp` index on the `timestamp` column already supports the primary access pattern (time-range filtering). For the GROUP BY queries (`model`, `provider`), SQLite will perform a table scan within the time range and then group. This is acceptable for a local proxy with moderate volume.

If performance becomes an issue with large datasets (unlikely for a local proxy), add:

```sql
CREATE INDEX idx_requests_model_timestamp ON requests(model, timestamp);
CREATE INDEX idx_requests_provider_timestamp ON requests(provider, timestamp);
```

**Do not add these indexes now.** Premature optimization for a local proxy. The timestamp index already narrows the scan. Measure first.

---

## Build Order

Implement in this sequence because each step depends on the previous:

1. **`src/storage/stats.rs`** -- Query functions and response types. No HTTP dependency. Independently testable with in-memory SQLite.
2. **`src/storage/mod.rs`** -- Add `pub mod stats;` declaration and re-exports.
3. **`src/proxy/handlers.rs`** -- Add query param structs (`TimeRangeParams`, `RecentParams`), helper function (`resolve_time_range`), and handler functions. Depends on storage::stats types.
4. **`src/proxy/server.rs`** -- Register new routes in `create_router()`. Depends on handler functions existing.
5. **Integration tests** -- Full HTTP tests against test server with pre-populated data.

### Testing Strategy

Unit tests for `storage/stats.rs` follow the exact pattern from `logging.rs` tests:

```rust
async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn test_get_summary_empty_db() {
    let pool = test_pool().await;
    let summary = get_summary(&pool, "2026-01-01T00:00:00Z", "2026-12-31T00:00:00Z")
        .await
        .unwrap();
    assert_eq!(summary.total_requests, 0);
    assert_eq!(summary.total_cost_sats, 0.0);
}
```

Insert test rows using the existing `RequestLog::insert()` method, then verify aggregates.

---

## Scalability Considerations

| Concern | Current (local proxy) | If volume grows |
|---------|----------------------|-----------------|
| Query latency | Sub-millisecond (SQLite + WAL, small dataset) | Add composite indexes, consider date partitioning |
| Connection contention | 5 pool connections, reads don't block writes in WAL | Increase pool size if needed |
| Response payload size | Dozens of rows in GROUP BY results | Add pagination for `/requests`, cap time ranges |
| Concurrent readers | No issue in WAL mode | No change needed |

---

## Sources

- Existing codebase: `src/proxy/server.rs`, `src/proxy/handlers.rs`, `src/storage/logging.rs`, `src/storage/mod.rs` -- **HIGH confidence** (direct inspection)
- [axum Query extractor docs](https://docs.rs/axum/latest/axum/extract/struct.Query.html) -- **HIGH confidence**
- [axum Router::nest docs](https://docs.rs/axum/latest/axum/routing/struct.Router.html) -- **HIGH confidence**
- [sqlx query_as documentation](https://docs.rs/sqlx/latest/sqlx/fn.query_scalar.html) -- **HIGH confidence**
- [SQLite WAL mode concurrency](https://www.sqlite.org/wal.html) -- **HIGH confidence** (SQLite official docs, already configured in `storage/mod.rs`)
- axum/sqlx integration patterns from community -- **MEDIUM confidence** (multiple sources agree on patterns)
