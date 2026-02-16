# Technology Stack: Cost Query API Endpoints

**Project:** arbstr - Read-only analytics/stats API endpoints
**Researched:** 2026-02-16
**Overall confidence:** HIGH

## Scope

This research covers ONLY the stack additions/changes needed for:
1. Aggregate SQL queries against the existing `requests` table (COUNT, SUM, AVG, GROUP BY)
2. Query parameter parsing for time ranges, model filters, and provider filters
3. JSON response serialization for stats data
4. axum route/extractor patterns for GET endpoints with optional query params

Everything else in the existing stack is unchanged and validated from v1/v1.1/v1.2.

## Existing Stack (No Changes Needed)

These dependencies remain correct and require no modifications:

| Technology | Version (in Cargo.toml) | Purpose for This Milestone | Status |
|------------|------------------------|---------------------------|--------|
| axum | 0.7 | HTTP server, route registration, `Query` extractor | Keep as-is |
| sqlx | 0.8 (sqlite, runtime-tokio, migrate) | Aggregate queries with `query_as`, `query_scalar` | Keep as-is |
| serde / serde_json | 1.x | Deserialize query params, serialize JSON responses | Keep as-is |
| chrono | 0.4 (serde feature) | Parse date strings from query params, UTC timestamp handling | Keep as-is |
| tokio | 1.x (full) | Async runtime | Keep as-is |
| tracing | 0.1 | Debug/info logging of query execution | Keep as-is |

**Critical finding: Zero new dependencies are needed.** The existing stack provides every capability required for read-only analytics endpoints.

## New Dependencies Required

### None.

The existing dependency set is sufficient for cost query API endpoints. Here is why each concern is already covered:

### 1. Aggregate SQL Queries: `sqlx` 0.8 (Already Present)

**Capability:** `sqlx::query_as::<_, T>()` with `#[derive(sqlx::FromRow)]` structs, and `sqlx::query_scalar()` for single-value aggregates.

**Pattern for aggregate queries:**

```rust
// Single aggregate value (e.g., total request count)
let count: i64 = sqlx::query_scalar(
    "SELECT COUNT(*) FROM requests WHERE timestamp >= ? AND timestamp < ?"
)
.bind(&start)
.bind(&end)
.fetch_one(pool)
.await?;

// Grouped aggregates (e.g., cost per model)
#[derive(sqlx::FromRow, serde::Serialize)]
struct ModelStats {
    model: String,
    request_count: i64,
    total_cost_sats: Option<f64>,
    avg_latency_ms: Option<f64>,
}

let stats: Vec<ModelStats> = sqlx::query_as(
    "SELECT model,
            COUNT(*) as request_count,
            SUM(cost_sats) as total_cost_sats,
            AVG(latency_ms) as avg_latency_ms
     FROM requests
     WHERE timestamp >= ? AND timestamp < ?
     GROUP BY model
     ORDER BY total_cost_sats DESC"
)
.bind(&start)
.bind(&end)
.fetch_all(pool)
.await?;
```

**Why `query_as` (runtime function) instead of `query_as!` (compile-time macro):**
- The compile-time macro (`query_as!`) requires a live DATABASE_URL at build time and has known issues with SQLite aggregate type inference (COUNT returns i32 not i64, GROUP BY can cause compilation hangs in older sqlx versions)
- The runtime function (`query_as::<_, T>()`) with `#[derive(sqlx::FromRow)]` works reliably, matches the existing codebase pattern (see `logging.rs` lines 86-96 using `sqlx::query()`), and avoids CI/build complexity
- The project already uses runtime `sqlx::query()` everywhere -- stay consistent

**SQLite aggregate type mapping:**

| SQL Function | SQLite Return | Rust Type to Use | Notes |
|-------------|---------------|------------------|-------|
| `COUNT(*)` | INTEGER | `i64` | Always non-NULL for COUNT(*) |
| `SUM(col)` | REAL or INTEGER | `Option<f64>` | NULL if all values are NULL or no rows |
| `AVG(col)` | REAL | `Option<f64>` | NULL if no rows match |
| `MIN(col)` / `MAX(col)` | Same as column | `Option<T>` | NULL if no rows match |
| `TOTAL(col)` | REAL | `f64` | Returns 0.0 instead of NULL (prefer over SUM for non-nullable result) |

**Confidence:** HIGH -- verified against [sqlx docs](https://docs.rs/sqlx/latest/sqlx/fn.query_as.html), [sqlx aggregate issues](https://github.com/launchbadge/sqlx/issues/3238), and existing codebase patterns.

### 2. Query Parameter Parsing: `axum::extract::Query` (Already Present)

**Capability:** `axum::extract::Query<T>` deserializes URL query strings into a typed struct using serde. Already available in axum 0.7.

**Pattern for stats endpoints:**

```rust
use axum::extract::Query;
use serde::Deserialize;

#[derive(Deserialize)]
struct StatsParams {
    /// Start of time range (ISO 8601 / RFC 3339, e.g., "2026-02-01T00:00:00Z")
    #[serde(default)]
    start: Option<String>,
    /// End of time range
    #[serde(default)]
    end: Option<String>,
    /// Filter by model name
    #[serde(default)]
    model: Option<String>,
    /// Filter by provider name
    #[serde(default)]
    provider: Option<String>,
}

async fn get_stats(
    State(state): State<AppState>,
    Query(params): Query<StatsParams>,
) -> Result<Json<StatsResponse>, Error> {
    // ...
}
```

**Why `Option<String>` for date params instead of `Option<chrono::DateTime<Utc>>`:**
- Chrono's `DateTime<Utc>` does implement `Deserialize`, but query string deserialization through serde_html_form (which axum uses internally) may not handle the `+` in `+00:00` correctly (URL encoding issue: `+` becomes space)
- Accepting `String` and parsing with `chrono::DateTime::parse_from_rfc3339()` in the handler gives better error messages and control
- The existing codebase stores timestamps as `chrono::Utc::now().to_rfc3339()` -- accepting the same format as query input is consistent

**Why NOT `axum-extra::extract::OptionalQuery`:**
- `OptionalQuery<T>` makes the entire query struct optional (returns None if no query string at all)
- We want individual optional fields, not an optional struct -- `#[serde(default)]` on `Option<T>` fields handles this perfectly
- No need for the `axum-extra` crate dependency

**Confidence:** HIGH -- verified against [axum Query extractor docs](https://docs.rs/axum/latest/axum/extract/struct.Query.html) and existing handler patterns in `handlers.rs`.

### 3. Time Range Handling: `chrono` 0.4 (Already Present)

**Capability:** Parse RFC 3339 date strings, compute defaults (e.g., "last 24 hours"), format for SQL WHERE clauses.

**Pattern for time range defaults and parsing:**

```rust
use chrono::{DateTime, Utc, Duration};

fn parse_time_range(start: Option<&str>, end: Option<&str>) -> Result<(String, String), Error> {
    let end_dt = match end {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map_err(|_| Error::BadRequest("Invalid end date (expected RFC 3339)".into()))?
            .with_timezone(&Utc),
        None => Utc::now(),
    };
    let start_dt = match start {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map_err(|_| Error::BadRequest("Invalid start date (expected RFC 3339)".into()))?
            .with_timezone(&Utc),
        None => end_dt - Duration::hours(24),
    };
    Ok((start_dt.to_rfc3339(), end_dt.to_rfc3339()))
}
```

**Why this works with SQLite text comparison:**
- All timestamps in the `requests` table are stored as RFC 3339 UTC strings (e.g., `2026-02-16T14:30:00.123456789+00:00`)
- RFC 3339 with consistent UTC timezone sorts lexicographically == chronologically
- `WHERE timestamp >= ? AND timestamp < ?` with string binding produces correct results
- No need for SQLite `datetime()` function or epoch conversion
- The existing `idx_requests_timestamp` index (from initial migration) makes range queries efficient

**Why NOT the `time` crate:**
- `chrono` 0.4 is already a direct dependency with the `serde` feature enabled
- Adding `time` would introduce a competing date/time library for zero benefit
- `chrono::DateTime::parse_from_rfc3339()` does exactly what we need

**Confidence:** HIGH -- verified timestamps are stored as `chrono::Utc::now().to_rfc3339()` (6 occurrences in `handlers.rs`), and [SQLite text comparison with ISO 8601](https://sqlite.org/lang_datefunc.html) confirms lexicographic ordering works for UTC timestamps.

### 4. JSON Response Serialization: `serde` + `serde_json` (Already Present)

**Capability:** `#[derive(Serialize)]` on response structs, `axum::Json<T>` for automatic serialization.

**Pattern for stats response types:**

```rust
use serde::Serialize;

#[derive(Serialize)]
struct StatsResponse {
    period: TimePeriod,
    totals: TotalStats,
    by_model: Vec<ModelStats>,
    by_provider: Vec<ProviderStats>,
}

#[derive(Serialize)]
struct TimePeriod {
    start: String,
    end: String,
}

#[derive(Serialize)]
struct TotalStats {
    total_requests: i64,
    successful_requests: i64,
    failed_requests: i64,
    total_cost_sats: f64,
    total_input_tokens: i64,
    total_output_tokens: i64,
    avg_latency_ms: f64,
}
```

**Why structs with `#[derive(Serialize)]` instead of `serde_json::json!()`:**
- The existing handlers use `serde_json::json!()` for simple responses (health, providers, models)
- Stats responses are more complex with nested structures -- typed structs prevent field name typos and make the API contract explicit
- `#[derive(sqlx::FromRow, serde::Serialize)]` on the same struct allows direct DB-to-JSON piping with no intermediate mapping
- Consistent with the existing `ChatCompletionResponse` pattern in `types.rs`

**Confidence:** HIGH -- standard serde pattern, already used extensively in the codebase.

## Recommended Stack Summary

### Core Framework (unchanged)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| axum | 0.7 | HTTP server, `Query<T>` extractor, `Json<T>` response, route registration | Already used, `Query` extractor built-in |
| sqlx | 0.8 | `query_as()` for aggregate SELECT, `query_scalar()` for single values, `FromRow` derive | Already used, runtime query functions match existing patterns |
| chrono | 0.4 | RFC 3339 parsing for time range params, UTC timestamp defaults | Already used for timestamp generation |
| serde / serde_json | 1.x | Deserialize query params, serialize JSON responses | Already used everywhere |

### Database (unchanged)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| SQLite via sqlx | 0.8 | Read-only aggregate queries against `requests` table | Already used, existing indexes cover time range queries |

### Infrastructure (unchanged)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| tokio | 1.x (full) | Async runtime for query execution | Already used |

### Supporting Libraries (unchanged)

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tracing | 0.1 | Log query execution times, parameter validation warnings | Every handler |

## What NOT to Add

| Technology | Why Not |
|------------|---------|
| `axum-extra` | `OptionalQuery<T>` is unnecessary -- `#[serde(default)]` on `Option<T>` fields handles optional query params. No need for a new dependency for one extractor. |
| `time` crate | Competing date/time library. `chrono` 0.4 is already present and sufficient. |
| `sqlx` compile-time macros (`query!`, `query_as!`) | Require DATABASE_URL at build time, have known SQLite aggregate type inference issues, and don't match the existing runtime query pattern used throughout the codebase. |
| `sea-query` / `diesel` | SQL query builder crates. The aggregate queries for stats are static (not user-constructed), so raw SQL with bind parameters is clearer and has no SQL injection risk. Adding a query builder for 5-6 fixed queries is over-engineering. |
| `chrono-tz` | Timezone database crate. All timestamps are UTC. No timezone conversion needed. |
| `serde_qs` / `serde_urlencoded` | Query string parsers. axum's built-in `Query<T>` extractor already uses `serde_html_form` internally, which handles all standard query string formats. |

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Date parsing | `chrono` 0.4 (existing) | `time` crate | Second date library for zero benefit; chrono already in deps |
| Query params | `axum::extract::Query` | `axum-extra::OptionalQuery` | Individual `Option` fields with `#[serde(default)]` is sufficient |
| SQL queries | `sqlx::query_as()` runtime | `sqlx::query_as!()` macro | Compile-time macro needs DATABASE_URL, has SQLite aggregate issues |
| SQL queries | Raw SQL with bind params | `sea-query` builder | Static queries don't benefit from a builder; adds complexity |
| Response types | Typed structs with Serialize | `serde_json::json!()` | Structs provide compile-time field name checking for complex responses |

## Integration Points with Existing Code

### New Files

| File | Purpose |
|------|---------|
| `src/proxy/stats.rs` (new) | Stats query handler functions and response types |

### Files That Change

| File | Change | Why |
|------|--------|-----|
| `src/proxy/server.rs` | Add GET routes for stats endpoints | Wire up new handlers to the router |
| `src/proxy/mod.rs` | Add `pub mod stats;` | Module registration |
| `src/storage/logging.rs` OR `src/storage/queries.rs` (new) | Add aggregate query functions | Separate read queries from write operations |

### Files That Don't Change

| File | Why Not |
|------|---------|
| `Cargo.toml` | No new dependencies |
| `src/config.rs` | No config changes needed |
| `src/router/` | Routing/selection logic unchanged |
| `src/error.rs` | Existing `BadRequest` variant covers invalid query params |
| `migrations/` | No schema changes -- existing table + indexes are sufficient |

### Existing Index Coverage

The initial migration already created:

```sql
CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);
```

This index supports efficient time-range filtering (`WHERE timestamp >= ? AND timestamp < ?`). For GROUP BY queries on `model` and `provider`, SQLite will scan the filtered result set. If performance becomes an issue at scale, composite indexes can be added later:

```sql
-- Only if needed (not for initial implementation)
CREATE INDEX idx_requests_model_timestamp ON requests(model, timestamp);
CREATE INDEX idx_requests_provider_timestamp ON requests(provider, timestamp);
```

**Recommendation:** Do NOT add these indexes now. The `requests` table is append-only with fire-and-forget writes. Adding indexes slows down every INSERT. Wait until query performance is measurably slow before optimizing.

## SQLite-Specific Considerations

### NULL Handling in Aggregates

- `SUM(cost_sats)` returns NULL if all `cost_sats` values in the group are NULL (streaming requests where usage extraction failed)
- Use `COALESCE(SUM(cost_sats), 0.0)` in SQL or `Option<f64>` in Rust and default to 0.0 in the response serialization
- `COUNT(*)` always returns a non-NULL integer (counts rows, not values)
- `COUNT(cost_sats)` counts only non-NULL values (useful for "requests with known cost")

### CAST for Integer Division

- `AVG(latency_ms)` returns REAL (f64) even though `latency_ms` is INTEGER -- this is correct behavior
- No explicit CAST needed for our use case

### Concurrent Reads During Writes

- SQLite WAL mode (default for sqlx) allows concurrent reads while writes are in progress
- Stats queries will not block the main proxy write path
- Read queries may not see the very latest fire-and-forget writes (acceptable for analytics)

## Cargo.toml Changes Summary

```toml
# NO CHANGES to [dependencies]
# Everything needed is already present:
# - axum = "0.7"                              (Query extractor, Json response, route registration)
# - sqlx = { features = ["sqlite", ...] }     (query_as, query_scalar, FromRow derive)
# - chrono = { features = ["serde"] }         (RFC 3339 parsing, UTC defaults)
# - serde = { features = ["derive"] }         (Deserialize for query params, Serialize for responses)
# - serde_json = "1"                          (JSON serialization)
# - tracing = "0.1"                           (Query logging)
```

**Net dependency change: 0.** Zero new crates. This milestone uses only existing dependencies.

## Version Verification

| Crate | Version (Cargo.toml) | Latest Stable | Action | Confidence |
|-------|---------------------|---------------|--------|------------|
| sqlx | 0.8 | 0.8.6 | Keep -- semver compatible, no breaking changes | HIGH |
| axum | 0.7 | 0.8.6 | Keep at 0.7 -- upgrading is out of scope for this milestone | HIGH |
| chrono | 0.4 | 0.4.43 | Keep -- semver compatible | HIGH |
| serde | 1.x | 1.x | Keep -- stable | HIGH |

**Note on axum 0.8:** The project currently uses axum 0.7. Axum 0.8 is available but upgrading is a separate concern (breaking changes in middleware, extractors). The `Query` extractor API is stable across both versions. Do not upgrade axum as part of this milestone.

## Sources

### Primary (HIGH confidence)
- Local codebase analysis: `Cargo.toml`, `src/proxy/handlers.rs`, `src/proxy/server.rs`, `src/storage/logging.rs`, `src/proxy/types.rs`, `migrations/*.sql` -- all read and verified
- [sqlx `query_as` function docs](https://docs.rs/sqlx/latest/sqlx/fn.query_as.html) -- runtime query_as with FromRow
- [sqlx `query_scalar` function docs](https://docs.rs/sqlx/latest/sqlx/fn.query_scalar.html) -- single-value aggregate queries
- [axum `Query` extractor docs](https://docs.rs/axum/latest/axum/extract/struct.Query.html) -- query string deserialization
- [SQLite Date And Time Functions](https://sqlite.org/lang_datefunc.html) -- ISO 8601 text comparison behavior
- [sqlx 0.8.6 on crates.io](https://crates.io/crates/sqlx) -- latest stable version confirmed
- [chrono 0.4.43 on docs.rs](https://docs.rs/crate/chrono/latest) -- latest stable version confirmed

### Secondary (MEDIUM confidence)
- [sqlx aggregate type issues (GitHub #3238)](https://github.com/launchbadge/sqlx/issues/3238) -- GROUP BY type inference problems with compile-time macros
- [axum-extra OptionalQuery docs](https://docs.rs/axum-extra/latest/axum_extra/extract/struct.OptionalQuery.html) -- confirmed unnecessary for this use case
- [axum 0.8 optional query params (GitHub #3079)](https://github.com/tokio-rs/axum/issues/3079) -- confirms `#[serde(default)]` pattern works
- [SQLite text comparison for timestamps](https://sqlite.work/resolving-date-comparison-and-ordering-issues-in-sqlite-with-non-standard-date-formats/) -- lexicographic ordering with ISO 8601
- [FromRow trait docs](https://docs.rs/sqlx/latest/sqlx/trait.FromRow.html) -- derive macro for struct mapping
