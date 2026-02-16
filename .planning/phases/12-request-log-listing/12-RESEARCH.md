# Phase 12: Request Log Listing - Research

**Researched:** 2026-02-16
**Domain:** Paginated SQLite record listing via axum + sqlx with filtering and sorting
**Confidence:** HIGH

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions

#### Response shape
- Curated field set -- exclude correlation_id, provider_cost_sats, and policy from response
- Included fields: id, timestamp, model, provider, streaming, input_tokens, output_tokens, cost_sats, latency_ms, stream_duration_ms, success, error_status, error_message
- Nested sections per record: group related fields (timing, costs, tokens) rather than flat objects
- Top-level wrapper includes pagination metadata AND effective time range (since/until)

#### Pagination style
- Page-based: `page` and `per_page` query params
- Default page size: 20, maximum: 100
- Response includes both `total` count and `total_pages` convenience field
- Out-of-range pages return 200 with empty data array (not 400)

#### Filter behavior
- Multiple filters combine with AND (all must match)
- Reuse same time range params as /v1/stats: `since`, `until`, `range` presets -- identical behavior
- Success filter: `success=true` or `success=false` (boolean query param)
- Non-existent model/provider values return 404 (consistent with /v1/stats)
- Streaming filter: `streaming=true` or `streaming=false`

#### Sort defaults
- Default: newest first (timestamp descending) when no sort param provided
- Param style: `sort=<field>&order=asc|desc` (two separate params)
- Valid sort fields: timestamp, cost_sats, latency_ms
- Invalid sort field returns 400 with list of valid options
- Single column sort only (no multi-column)

### Claude's Discretion
- Exact nested section names and structure within each record
- How to handle the default time range for logs (whether to default to last_7d like stats or show all)
- Error response format details

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope

</user_constraints>

## Summary

Phase 12 adds a `GET /v1/requests` endpoint that returns paginated, filtered, sortable individual request records from the existing `requests` SQLite table. This is a read-only list endpoint building directly on infrastructure established in Phase 11: the read-only connection pool (`read_db` in `AppState`), the `resolve_time_range()` function in `proxy/stats.rs`, the `exists_in_db()` validation function in `storage/stats.rs`, the existing `Error::NotFound` and `Error::BadRequest` error variants, and the `tower::ServiceExt::oneshot` integration test pattern.

The core technical challenge is a paginated SELECT query with dynamic WHERE clauses and dynamic ORDER BY. The WHERE clause is built by combining time range, model, provider, success, and streaming filters with AND logic. The ORDER BY uses a whitelisted sort column to prevent SQL injection (same pattern Phase 11 used for `group_by` validation). Pagination is implemented as a two-query approach: one `COUNT(*)` query for total, then a `SELECT ... LIMIT ? OFFSET ?` query for the page data. This avoids the `COUNT(*) OVER()` window function pitfall where out-of-range OFFSET returns zero rows (and therefore zero total count).

Zero new dependencies are required. The existing stack (axum 0.7, sqlx 0.8, chrono 0.4, serde, serde_json) covers everything. The new code follows established patterns: a new `LogsQuery` serde struct for query params, a new handler function, new storage query functions, and integration tests using the `oneshot` pattern from Phase 11.

**Primary recommendation:** Build a `logs_handler` in a new `proxy/logs.rs` module. Reuse `resolve_time_range()` and `exists_in_db()` from Phase 11. Create `storage/logs.rs` with a `query_logs()` and `count_logs()` function pair. Register at `/v1/requests`. Default time range: `last_7d` (consistent with stats). Nested record sections: `timing`, `tokens`, `cost`.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| axum | 0.7 | HTTP server, `Query<T>` extractor for query params | Already used, auto-400 on malformed params |
| sqlx | 0.8 | SQLite queries with `?` bind params, `query_as` for typed results | Already used, read-only pool already initialized |
| chrono | 0.4 | Time range resolution via `resolve_time_range()` (reuse from Phase 11) | Already used |
| serde | 1 | `Deserialize` for query params, `Serialize` for response structs | Already used |
| serde_json | 1 | JSON response serialization | Already used |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tracing | 0.1 | Debug logging for query params, SQL execution | Already used |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Two-query pagination (COUNT + SELECT) | `COUNT(*) OVER()` window function | OVER() returns 0 total when OFFSET exceeds row count; two queries are simpler and always correct |
| String-built dynamic SQL | `sqlx::QueryBuilder` | QueryBuilder adds complexity; at most 6 filter clauses, string concat with hardcoded SQL fragments is simpler and safe |
| `page_hunter` / `sqlx-paginated` crate | Hand-built pagination | These crates exist but add unnecessary dependencies for a simple LIMIT/OFFSET with 2 queries |

**Installation:**
```bash
# No new dependencies needed. All libraries already in Cargo.toml.
```

## Architecture Patterns

### Recommended Project Structure
```
src/
├── proxy/
│   ├── mod.rs           # Add `pub mod logs;`, export logs_handler
│   ├── handlers.rs      # Add `pub use super::logs::logs_handler as logs;`
│   ├── stats.rs         # REUSE: resolve_time_range(), RangePreset (already pub)
│   ├── logs.rs          # NEW: LogsQuery, LogsResponse, LogEntry, logs_handler
│   └── server.rs        # Add route: .route("/v1/requests", get(handlers::logs))
├── storage/
│   ├── mod.rs           # Add `pub mod logs;`, export query functions
│   ├── stats.rs         # REUSE: exists_in_db() for model/provider 404 validation
│   └── logs.rs          # NEW: query_logs(), count_logs()
└── error.rs             # No changes needed (NotFound, BadRequest already exist)
tests/
└── logs.rs              # NEW: Integration tests mirroring tests/stats.rs pattern
```

### Pattern 1: Query Parameter Struct with Optional Filters
**What:** Define a `LogsQuery` struct with all query params as `Option<T>`. Axum's `Query<T>` extractor handles deserialization; absent params become `None`.
**When to use:** For the `/v1/requests` handler.
**Example:**
```rust
// Source: axum docs + existing StatsQuery pattern in proxy/stats.rs
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    // Time range (reuse stats behavior)
    pub range: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    // Filters
    pub model: Option<String>,
    pub provider: Option<String>,
    pub success: Option<bool>,
    pub streaming: Option<bool>,
    // Pagination
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    // Sorting
    pub sort: Option<String>,
    pub order: Option<String>,
}
```

### Pattern 2: Sort Column Whitelisting
**What:** Validate the `sort` param against a hardcoded whitelist of allowed column names. Map user-facing names to SQL column names. Return 400 with valid options on invalid input.
**When to use:** Before building the ORDER BY clause.
**Example:**
```rust
// Source: Phase 11 pattern (match statement for column whitelisting)
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
```

### Pattern 3: Two-Query Pagination
**What:** Execute two queries: a COUNT(*) query with the same filters (no LIMIT/OFFSET), then a SELECT query with LIMIT/OFFSET. Compute `total_pages = ceil(total / per_page)`.
**When to use:** For all paginated list endpoints.
**Example:**
```rust
// Count total matching rows (no pagination)
let total = count_logs(pool, &since_str, &until_str, filters).await?;
let total_pages = (total as f64 / per_page as f64).ceil() as u32;

// Fetch the page
let offset = (page - 1) * per_page;
let rows = query_logs(pool, &since_str, &until_str, filters, sort_col, sort_dir, per_page, offset).await?;
```

### Pattern 4: Nested Record Response Structure
**What:** Group related fields in each log record into nested objects: `timing` (latency, stream_duration), `tokens` (input, output), `cost` (cost_sats). Top-level fields: id, timestamp, model, provider, streaming, success, error.
**When to use:** Serializing individual log entries.
**Example:**
```rust
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct LogEntry {
    pub id: i64,
    pub timestamp: String,
    pub model: String,
    pub provider: Option<String>,
    pub streaming: bool,
    pub success: bool,
    pub tokens: TokensSection,
    pub cost: CostSection,
    pub timing: TimingSection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorSection>,
}

#[derive(Debug, Serialize)]
pub struct TokensSection {
    pub input: Option<i64>,
    pub output: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CostSection {
    pub sats: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct TimingSection {
    pub latency_ms: i64,
    pub stream_duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ErrorSection {
    pub status: Option<i32>,
    pub message: Option<String>,
}
```

### Pattern 5: Wrapper Response with Pagination Metadata
**What:** Top-level response includes `data` array, pagination fields, and effective time range.
**When to use:** The final JSON response shape.
**Example:**
```rust
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
```

### Pattern 6: Reuse Time Range and Filter Validation from Stats
**What:** Call `resolve_time_range()` from `proxy/stats.rs` and `exists_in_db()` from `storage/stats.rs` directly. No duplication.
**When to use:** In the logs handler before building queries.
**Example:**
```rust
// In proxy/logs.rs
use super::stats::resolve_time_range;
use crate::storage::stats::exists_in_db;

// Resolve time range (identical to stats behavior)
let (since_dt, until_dt) = resolve_time_range(
    params.range.as_deref(),
    params.since.as_deref(),
    params.until.as_deref(),
)?;

// Validate model filter (404 for non-existent, same as stats)
if let Some(ref model_filter) = params.model {
    let in_config = state.config.providers.iter().any(|p| {
        p.models.iter().any(|m| m.eq_ignore_ascii_case(model_filter))
    });
    if !in_config {
        let in_db = exists_in_db(pool, "model", model_filter).await?;
        if !in_db {
            return Err(Error::NotFound(format!("Model '{}' not found", model_filter)));
        }
    }
}
```

### Anti-Patterns to Avoid
- **String-interpolating sort columns into SQL:** The sort column comes from user input. MUST use a whitelist match statement, not `format!("ORDER BY {}", user_input)`. The match returns a `&'static str` constant.
- **Using COUNT(*) OVER() for pagination total:** When the page OFFSET exceeds total rows, the result set is empty and `COUNT(*) OVER()` returns no rows -- so you get zero total. Use a separate COUNT query instead.
- **Returning 400 for out-of-range pages:** User decision explicitly requires 200 with empty `data` array for out-of-range pages. Do NOT return 400.
- **Sharing mutable SQL builder state:** Build the WHERE clause dynamically using string concat with hardcoded SQL fragments (safe) and `?` bind params (safe). Never interpolate user values.
- **Duplicating time range resolution logic:** Reuse `resolve_time_range()` from `proxy/stats.rs` -- it is already `pub`.
- **Using LIMIT without ORDER BY:** Undefined behavior in SQL. Always include ORDER BY before LIMIT/OFFSET.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Query param parsing | Manual URL parsing | `axum::extract::Query<T>` with serde `Deserialize` | Auto-400 on malformed, handles Option types |
| Time range resolution | Custom date parsing | `resolve_time_range()` from `proxy/stats.rs` | Already tested, handles presets/explicit/defaults |
| Model/provider existence checks | Custom validation | `exists_in_db()` from `storage/stats.rs` | Already tested, case-insensitive, SQL-injection-safe |
| Pagination math | Manual offset calc | `(page - 1) * per_page` + `ceil(total / per_page)` | Simple arithmetic, but easy to off-by-one |
| JSON nesting | Manual string building | `#[derive(Serialize)]` structs | Type-safe, handles Option skipping |
| Timestamp parsing | Regex / string splitting | `chrono::DateTime::parse_from_rfc3339()` | Handles timezones, validation |

**Key insight:** Phase 12 reuses substantial infrastructure from Phase 11. The main new work is the paginated SELECT query with dynamic WHERE + ORDER BY, the nested response struct serialization, and integration tests. Everything else is reuse.

## Common Pitfalls

### Pitfall 1: SQL Injection via Sort Column
**What goes wrong:** `format!("ORDER BY {} {}", sort_field, order)` allows SQL injection if `sort_field` comes from user input.
**Why it happens:** Parameterized queries (`?` binds) cannot be used for column names or ORDER BY direction -- they are structural SQL elements, not values.
**How to avoid:** Use a whitelist match statement that returns `&'static str`. The match guarantees only known column names reach the SQL string.
**Warning signs:** Using `format!()` with user-provided strings for ORDER BY.

### Pitfall 2: Off-by-One in Page Calculation
**What goes wrong:** Page 1 should return rows 0-19, not 1-20. `OFFSET = page * per_page` skips the first page.
**Why it happens:** 1-indexed pages vs 0-indexed SQL OFFSET.
**How to avoid:** `offset = (page - 1) * per_page`. Page 1 -> offset 0. Page 2 -> offset 20.
**Warning signs:** First page missing expected records.

### Pitfall 3: total_pages Calculation with Integer Division
**What goes wrong:** `total / per_page` truncates. 21 rows with per_page=20 gives 1 page instead of 2.
**Why it happens:** Rust integer division truncates toward zero.
**How to avoid:** Use ceiling division: `(total + per_page - 1) / per_page` or cast to f64 and use `.ceil()`. When total is 0, result is 0 pages.
**Warning signs:** Last partial page of results unreachable.

### Pitfall 4: LIMIT/OFFSET Without ORDER BY
**What goes wrong:** SQL does not guarantee row ordering without ORDER BY. Different pages may return overlapping or missing rows.
**Why it happens:** SQLite's natural order is the rowid insertion order, but this is not guaranteed by the SQL standard and can change with VACUUM or index changes.
**How to avoid:** Always include ORDER BY before LIMIT/OFFSET. The default is `ORDER BY timestamp DESC`.
**Warning signs:** Flaky tests where pagination returns inconsistent results.

### Pitfall 5: Boolean Query Param Deserialization
**What goes wrong:** `serde_urlencoded` deserializes `?success=true` to `bool` correctly, but `?success=1` or `?success=yes` will fail with a 400 error.
**Why it happens:** Serde's bool deserializer expects "true" or "false" strings.
**How to avoid:** Document that only `true`/`false` are accepted. The 400 from axum's Query extractor is the correct behavior -- no special handling needed. Using `Option<bool>` means an absent param is `None`, and `?success=true` or `?success=false` both work.
**Warning signs:** Users getting 400 when using `1`/`0` for boolean params.

### Pitfall 6: Nullable Fields in Query Results
**What goes wrong:** `provider`, `input_tokens`, `output_tokens`, `cost_sats`, `stream_duration_ms`, `error_status`, `error_message` are all nullable in the database. Using non-Option types in the Rust struct causes sqlx to panic at runtime.
**Why it happens:** The schema allows NULL for these columns (failed requests have no provider, no tokens, no cost).
**How to avoid:** Use `Option<T>` for every nullable column in the `sqlx::FromRow` struct. Map them to the response struct's Option fields.
**Warning signs:** Runtime panic: "expected non-null value for column 'provider'".

### Pitfall 7: Case-Insensitive Filter Matching
**What goes wrong:** A request logged as "GPT-4o" does not match `?model=gpt-4o` without LOWER().
**Why it happens:** SQLite text comparison is case-sensitive by default.
**How to avoid:** Use `LOWER(model) = LOWER(?)` in WHERE clause (same as Phase 11). Accept the index miss tradeoff -- local proxy scale is small.
**Warning signs:** Filters silently returning fewer results than expected.

### Pitfall 8: Empty Data on Out-of-Range Page
**What goes wrong:** Returning 400 instead of 200 with empty data for page numbers beyond total_pages.
**Why it happens:** Natural instinct is to validate page range.
**How to avoid:** User decision explicitly says: out-of-range pages return 200 with empty data array. The SQL query with large OFFSET simply returns zero rows. Let it happen naturally.
**Warning signs:** Tests expecting 200 getting 400.

## Code Examples

Verified patterns from existing codebase and official sources:

### Dynamic WHERE Clause Builder for Logs
```rust
// Source: Pattern established in storage/stats.rs query_aggregate()
pub async fn count_logs(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
    success: Option<bool>,
    streaming: Option<bool>,
) -> Result<i64, sqlx::Error> {
    let mut sql = String::from(
        "SELECT COUNT(*) as cnt FROM requests WHERE timestamp >= ? AND timestamp <= ?"
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

    let mut query = sqlx::query_scalar::<_, i64>(&sql)
        .bind(since)
        .bind(until);

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
```

### Paginated SELECT with Dynamic ORDER BY
```rust
// Source: Pattern from storage/stats.rs + sort whitelisting from Phase 11 research
pub async fn query_logs(
    pool: &SqlitePool,
    since: &str,
    until: &str,
    model: Option<&str>,
    provider: Option<&str>,
    success: Option<bool>,
    streaming: Option<bool>,
    sort_column: &str,    // Already validated via whitelist
    sort_direction: &str, // Already validated: "ASC" or "DESC"
    limit: u32,
    offset: u32,
) -> Result<Vec<LogRow>, sqlx::Error> {
    let mut sql = String::from(
        "SELECT id, timestamp, model, provider, streaming, \
         input_tokens, output_tokens, cost_sats, \
         latency_ms, stream_duration_ms, \
         success, error_status, error_message \
         FROM requests WHERE timestamp >= ? AND timestamp <= ?"
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

    // Safe: sort_column and sort_direction are &'static str from whitelist
    sql.push_str(&format!(" ORDER BY {} {}", sort_column, sort_direction));
    sql.push_str(" LIMIT ? OFFSET ?");

    let mut query = sqlx::query_as::<_, LogRow>(&sql)
        .bind(since)
        .bind(until);

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
```

### LogRow sqlx::FromRow Struct
```rust
// Source: Schema from migrations/20260203000000_initial_schema.sql
// All nullable columns use Option<T>
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
```

### Integration Test Pattern (from tests/stats.rs)
```rust
// Source: tests/stats.rs -- reuse setup_test_app, seed_request, get, parse_response
use tower::ServiceExt; // for oneshot

async fn get(app: axum::Router, uri: &str) -> (http::StatusCode, serde_json::Value) {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    parse_response(response).await
}

#[tokio::test]
async fn test_logs_default_page() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/requests").await;

    assert_eq!(status, 200);
    assert!(body["data"].is_array());
    assert_eq!(body["per_page"], 20);
    assert_eq!(body["page"], 1);
    assert!(body["total"].is_number());
    assert!(body["total_pages"].is_number());
    assert!(body["since"].is_string());
    assert!(body["until"].is_string());
}
```

## Discretionary Recommendations

### Nested Record Structure (Claude's Discretion)

Recommend the following nested section names for individual log entries:

| Section | Fields | Rationale |
|---------|--------|-----------|
| `tokens` | `input`, `output` | Short, clear. Mirrors "input_tokens" / "output_tokens" without redundancy. |
| `cost` | `sats` | Singular since there is only one cost field exposed (cost_sats). |
| `timing` | `latency_ms`, `stream_duration_ms` | Groups all time-related metrics. |
| `error` | `status`, `message` | Present only when `success=false` (use `skip_serializing_if = "Option::is_none"`). |

Top-level fields (not nested): `id`, `timestamp`, `model`, `provider`, `streaming`, `success`.

This structure means a successful non-streaming request looks like:
```json
{
  "id": 42,
  "timestamp": "2026-02-16T12:00:00+00:00",
  "model": "gpt-4o",
  "provider": "alpha",
  "streaming": false,
  "success": true,
  "tokens": {"input": 100, "output": 200},
  "cost": {"sats": 10.0},
  "timing": {"latency_ms": 150, "stream_duration_ms": null}
}
```

And a failed request includes the error section:
```json
{
  "id": 43,
  "success": false,
  "tokens": {"input": null, "output": null},
  "cost": {"sats": null},
  "timing": {"latency_ms": 500, "stream_duration_ms": null},
  "error": {"status": 502, "message": "Provider returned 502"}
}
```

### Default Time Range for Logs (Claude's Discretion)

Recommend: **Default to `last_7d`** (same as stats). Rationale:
1. Consistency with `/v1/stats` -- users learn one set of defaults.
2. "Show all" is dangerous with unbounded data -- no default time window could return thousands of records across many pages.
3. Users can easily expand with `range=last_30d` or explicit `since`/`until` if needed.

### Error Response Format (Claude's Discretion)

Reuse the existing OpenAI-compatible error format from `Error::into_response()` in `error.rs`:
```json
{
  "error": {
    "message": "Invalid sort field 'invalid'. Valid options: timestamp, cost_sats, latency_ms",
    "type": "arbstr_error",
    "code": 400
  }
}
```

No changes needed -- `Error::BadRequest` and `Error::NotFound` already produce this format.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `COUNT(*) OVER()` for pagination | Two-query: COUNT then SELECT | Always been the safer pattern | Avoids zero-total on out-of-range offset |
| Flat response with all DB columns | Curated fields with nested sections | API design best practice | Hides internal columns (correlation_id, provider_cost_sats, policy) |

**Deprecated/outdated:**
- None relevant -- all libraries in use are current stable versions.

## Open Questions

1. **Should `error` section appear on success=true records?**
   - What we know: Some successful streaming requests have `error_message = "client_disconnected"` with `success = true`. The `error_status` would be NULL.
   - What's unclear: Whether to show the error section on these edge cases.
   - Recommendation: Only include `error` section when `error_status IS NOT NULL OR error_message IS NOT NULL`. This captures both failed requests and successful-but-notable requests (client disconnect). Use `skip_serializing_if = "Option::is_none"` on the outer `Option<ErrorSection>`.

2. **Shared filter builder between count and select queries**
   - What we know: Both `count_logs()` and `query_logs()` build identical WHERE clauses.
   - What's unclear: Whether to extract a shared filter builder or accept the duplication.
   - Recommendation: Extract a shared `build_filter_clause()` function that returns `(String, Vec<sqlx::SqliteArguments>)` or accept the duplication since the filter logic is small (6 possible clauses). The duplication approach is simpler and follows the existing Phase 11 pattern where `query_aggregate` and `query_grouped_by_model` share similar WHERE logic without abstraction.

3. **sqlx bind order for dynamic queries**
   - What we know: `sqlx` binds are positional (`?`). The order of `.bind()` calls must match the order of `?` in the SQL string.
   - What's unclear: Nothing -- this is well understood from Phase 11.
   - Recommendation: Build filters with a consistent pattern: time range first, then model, provider, success, streaming in that order. Match `.bind()` order exactly. Both count and select queries must use the same order.

## Sources

### Primary (HIGH confidence)
- Existing codebase: `src/proxy/stats.rs` -- `resolve_time_range()`, `StatsQuery` pattern, handler structure
- Existing codebase: `src/storage/stats.rs` -- `exists_in_db()`, dynamic WHERE clause building pattern, `sqlx::FromRow`
- Existing codebase: `tests/stats.rs` -- `setup_test_app()`, `seed_request()`, `get()` helper, `oneshot` test pattern
- Existing codebase: `migrations/20260203000000_initial_schema.sql` -- Full schema with nullable column definitions
- [axum::extract::Query docs](https://docs.rs/axum/latest/axum/extract/struct.Query.html) -- Query parameter extraction, serde_urlencoded deserialization
- [SQLite aggregate docs](https://sqlite.org/lang_aggfunc.html) -- COUNT, TOTAL, AVG behavior

### Secondary (MEDIUM confidence)
- [Baeldung: LIMIT/OFFSET with total count](https://www.baeldung.com/sql/limit-offset-include-total-row-count) -- Confirmed two-query approach vs COUNT(*) OVER() tradeoffs
- WebSearch verification: `serde_urlencoded` handles `Option<bool>` correctly for `?success=true`/`?success=false`

### Tertiary (LOW confidence)
- None -- all findings verified with primary sources (existing codebase) or official docs.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - zero new dependencies, all reused from Phase 11
- Architecture: HIGH - patterns directly follow Phase 11 conventions; new code mirrors existing stats module structure
- Pitfalls: HIGH - all pitfalls verified against existing codebase behavior and Phase 11 experience

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable domain, no fast-moving dependencies)
