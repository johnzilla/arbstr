# Domain Pitfalls: Cost Querying API Endpoints

**Domain:** Adding read-only analytics/stats query endpoints to an existing SQLite-backed Rust proxy
**Researched:** 2026-02-16
**Scope:** SQL aggregation over nullable columns, f64 precision in cost sums, TEXT timestamp indexing and range queries, async SQLite contention, query parameter validation, response format consistency
**Confidence:** HIGH (based on direct codebase analysis, SQLite official documentation, sqlx behavior, chrono output format verification)

---

## Critical Pitfalls

Mistakes that produce silently wrong numbers, degrade proxy performance, or break existing endpoints.

---

### Pitfall 1: SUM(cost_sats) Returns NULL When All Rows Have NULL Cost

**What goes wrong:** The `cost_sats` column is `REAL` and nullable. Failed requests, streaming requests where usage was not reported, and requests where the provider did not include token counts all have `cost_sats = NULL`. When a query filters to a time range or model where ALL rows have NULL cost (e.g., a model that only serves streaming without `stream_options`), `SUM(cost_sats)` returns NULL, not 0.

This NULL propagates through any arithmetic: `SUM(cost_sats) + SUM(provider_cost_sats)` becomes NULL if either SUM is NULL. The JSON response then contains `null` where the consumer expects a number, or worse, `serde_json` serializes `Option<f64>::None` as `null` which JavaScript clients may render as `"null"` or `NaN` depending on how they parse it.

**Why it happens:** SQL standard requires `SUM()` to return NULL for empty or all-NULL groups. Developers test with seed data that has non-NULL costs and never encounter this edge.

**Consequences:**
- API returns `null` for `total_cost_sats` in periods with only failed requests
- Downstream dashboards show "NaN" or "undefined" instead of "$0.00"
- Arithmetic expressions like `(total_cost - provider_cost)` silently produce NULL even when one side has real data
- JSON consumers that do `response.total_cost.toFixed(2)` crash on null

**Prevention:**
1. Use SQLite's `TOTAL()` function instead of `SUM()` for cost aggregations. `TOTAL()` returns 0.0 for empty/all-NULL groups instead of NULL. It always returns a float, which matches the `REAL` column type.
2. For counts of requests with cost data, use `COUNT(cost_sats)` (counts non-NULL) alongside `COUNT(*)` (counts all rows). This tells the consumer "47 of 100 requests had cost data."
3. In the Rust response struct, use `f64` (not `Option<f64>`) for aggregate cost fields, since `TOTAL()` always returns a value.
4. Wrap any remaining `SUM()` calls with `COALESCE(SUM(x), 0.0)` as a defensive pattern.

**SQL example (wrong vs right):**
```sql
-- WRONG: Returns NULL when no rows have cost data
SELECT SUM(cost_sats) as total_cost FROM requests WHERE model = 'gpt-4o' AND timestamp >= '2026-02-01';

-- RIGHT: Returns 0.0 when no rows have cost data
SELECT TOTAL(cost_sats) as total_cost FROM requests WHERE model = 'gpt-4o' AND timestamp >= '2026-02-01';
```

**Phase to address:** First implementation phase -- every aggregate query must use the correct function from the start. Retrofitting is cheap but the silent NULL bugs are hard to detect.

**Confidence:** HIGH -- `SUM()` vs `TOTAL()` behavior is [documented by SQLite](https://sqlite.org/lang_aggfunc.html) and the `cost_sats` column is demonstrably nullable in the schema and in practice (visible in `RequestLog` struct at `storage/logging.rs:18`).

---

### Pitfall 2: AVG(cost_sats) Excludes NULL Rows Silently -- Misleading Averages

**What goes wrong:** `AVG()` in SQLite computes `TOTAL() / COUNT(column)`, which means it only considers non-NULL values. If 80 out of 100 requests have `cost_sats = NULL` (streaming without usage), `AVG(cost_sats)` computes the average of the 20 that DO have costs. This is mathematically correct but semantically misleading: the consumer sees "average cost per request = 15.3 sats" without knowing this represents only 20% of traffic.

The same issue affects `AVG(input_tokens)`, `AVG(output_tokens)`, and `AVG(latency_ms)` if those columns ever contain NULL. Currently `latency_ms` is `NOT NULL` so it is safe, but `input_tokens` and `output_tokens` are nullable.

**Why it happens:** SQL's NULL-skipping behavior in aggregates is by design, but consumers of API responses rarely understand this nuance. A dashboard showing "avg tokens: 500" does not reveal whether this represents 100% or 10% of requests.

**Consequences:**
- Average cost appears artificially high (because cheap/failed NULL-cost requests are excluded from the denominator)
- Average token counts are skewed toward successful completions, hiding the error rate
- Business decisions based on misleading metrics

**Prevention:**
1. Always return both the aggregate value AND the count of non-NULL values alongside the total count:
   ```sql
   SELECT
       AVG(cost_sats) as avg_cost_sats,
       COUNT(cost_sats) as requests_with_cost,
       COUNT(*) as total_requests
   FROM requests WHERE ...
   ```
2. Consider offering two averages: "average cost per request" (using `TOTAL(cost_sats) / COUNT(*)` to treat NULLs as zero) and "average cost per billed request" (using `AVG(cost_sats)` on non-NULL rows only). Document which one each endpoint returns.
3. In the API response, include a `sample_size` or `coverage` field so consumers know how representative the average is.

**Phase to address:** API design phase -- the response schema must include coverage/sample information. This cannot be added later without breaking the response format.

**Confidence:** HIGH -- `AVG()` NULL-skipping is [SQLite standard behavior](https://sqlite.org/lang_aggfunc.html). The `input_tokens` and `output_tokens` columns are nullable (schema at `migrations/20260203000000_initial_schema.sql:10-11`).

---

### Pitfall 3: Floating-Point Precision Loss in TOTAL/SUM of cost_sats

**What goes wrong:** `cost_sats` is stored as `REAL` (IEEE 754 double-precision float). When summing thousands of small cost values (e.g., 0.08 sats per request), floating-point addition accumulates rounding errors. SQLite's `SUM()` and `TOTAL()` use simple sequential addition, not [Kahan compensated summation](https://sqlite.org/forum/info/a0b458d8e). For arbstr's typical use case (satoshi-level costs, thousands of requests), the error is bounded:

- 10,000 requests at ~0.08 sats each: true sum = 800.0, float sum error < 0.001 sats
- 1,000,000 requests: error could reach ~0.1 sats

This is unlikely to matter for arbstr's use case (cost tracking, not billing), but becomes a pitfall if someone treats these numbers as exact for invoicing.

**Why it happens:** IEEE 754 binary floats cannot represent many decimal values exactly (e.g., 0.1 is actually 0.1000000000000000055511151231257827021181583404541015625). Each addition compounds the error.

**Consequences:**
- Summed costs differ slightly from expected values (e.g., 799.9999999999994 instead of 800.0)
- Displayed values show ugly trailing decimals ("42.350000000000001")
- If comparing sums for equality (e.g., reconciliation), float comparison fails

**Prevention:**
1. Round output values in the SQL query or Rust handler:
   ```sql
   SELECT ROUND(TOTAL(cost_sats), 2) as total_cost_sats FROM requests WHERE ...
   ```
   Or in Rust: `(total_cost * 100.0).round() / 100.0`
2. Format with fixed decimal places in the JSON response: `format!("{:.2}", cost)` (already done for the `x-arbstr-cost-sats` header at `handlers.rs:112`). Apply the same formatting to analytics responses.
3. Do NOT use `CAST(cost_sats * 100 AS INTEGER)` for "integer cents" -- this truncates rather than rounds and is a different class of bug.
4. Document that cost values are approximate (floating-point) and suitable for analytics, not invoicing.
5. If exact arithmetic is ever needed, store costs as integer sub-satoshis (e.g., millsats as `INTEGER`) in a future migration. But for the current analytics use case, rounding to 2 decimal places is sufficient.

**Phase to address:** Implementation phase -- apply `ROUND()` or Rust-side formatting to all cost output. This is a one-line fix per query.

**Confidence:** HIGH -- [SQLite float precision is documented](https://sqlite.org/floatingpoint.html) and the `cost_sats` column is `REAL` (schema line 12). The error magnitude is well-understood for IEEE 754.

---

### Pitfall 4: Analytics Queries Block the Proxy's Write Path via SQLite Locking

**What goes wrong:** The current pool has `max_connections(5)` (storage/mod.rs:21-22). All 5 connections serve both reads (analytics queries) and writes (fire-and-forget INSERTs and UPDATEs from the proxy path). SQLite in WAL mode allows concurrent reads, but a long-running analytics query (e.g., `SELECT ... FROM requests WHERE timestamp >= '2026-01-01' GROUP BY model, provider`) that scans tens of thousands of rows can:

1. **Hold a read transaction open**, preventing WAL checkpointing. The WAL file grows unboundedly while the query runs.
2. **Monopolize a connection from the pool.** With 5 connections and a slow analytics query holding one, only 4 remain for proxy traffic. If two analytics queries run simultaneously, only 3 connections serve the proxy.
3. **Trigger pool exhaustion.** If the analytics query takes 5 seconds and multiple clients hit analytics endpoints concurrently, all pool connections are consumed by reads. Proxy writes start queuing. The `spawn_log_write` fire-and-forget pattern means the write tasks queue up, holding `SqlitePool::acquire()` futures. If the pool is fully occupied for long enough, proxy requests start timing out.

**Why it happens:** The pool was sized for fire-and-forget writes (fast, non-blocking, WAL-friendly). Analytics queries are a fundamentally different workload: slow, table-scanning reads that hold connections for seconds instead of milliseconds.

**Consequences:**
- Analytics queries compete with proxy writes for the same small connection pool
- WAL file grows during long-running analytics queries, slowing all operations
- Under concurrent analytics load, proxy requests may fail with pool timeout errors
- Fire-and-forget log writes silently fail because they cannot acquire a connection

**Prevention:**
1. **Use a separate read-only connection pool for analytics queries.** Create a second `SqlitePool` with `max_connections(2)` opened with `read_only(true)` on the `SqliteConnectOptions`. Pass this pool to analytics handlers, keep the existing pool for writes. Read-only connections in WAL mode never conflict with writes.
   ```rust
   let read_opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
       .journal_mode(SqliteJournalMode::Wal)
       .read_only(true);
   let read_pool = SqlitePoolOptions::new()
       .max_connections(2)
       .connect_with(read_opts)
       .await?;
   ```
2. **Add query timeouts for analytics.** Use `sqlx::query(...).fetch_one_with(pool, timeout)` or wrap in `tokio::time::timeout()` to prevent analytics queries from holding connections indefinitely.
3. **Do NOT increase the write pool size** to accommodate analytics. SQLite's single-writer model means extra write connections just contend on the write lock. Keep the write pool small (2-5) and add a separate read pool.
4. **Consider adding `LIMIT` and pagination to all analytics endpoints** to bound query execution time.

**Phase to address:** Infrastructure phase (before implementing endpoints). The read pool setup must exist before analytics handlers are written, or every handler will incorrectly use the write pool.

**Confidence:** HIGH -- SQLite WAL concurrency behavior is [well-documented](https://sqlite.org/wal.html). The current pool setup is at `storage/mod.rs:21-22` with 5 connections. The fire-and-forget write pattern at `storage/logging.rs:59-70` is observable.

---

### Pitfall 5: Timestamp Range Queries Failing Due to Inconsistent RFC 3339 Formatting

**What goes wrong:** The `timestamp` column stores TEXT values produced by `chrono::Utc::now().to_rfc3339()`. This function uses chrono's "autoformat" for subsecond precision: it emits variable-length nanosecond digits depending on the actual value. This produces timestamps like:

- `2026-02-16T10:30:00+00:00` (no subseconds when nanoseconds = 0)
- `2026-02-16T10:30:00.123+00:00` (3 digits)
- `2026-02-16T10:30:00.123456+00:00` (6 digits)
- `2026-02-16T10:30:00.123456789+00:00` (9 digits)

The existing `idx_requests_timestamp` index enables B-tree range scans on these TEXT values using SQLite's BINARY collation (byte-by-byte comparison). For range queries like `WHERE timestamp >= '2026-02-01' AND timestamp < '2026-03-01'`, this works correctly because ISO 8601/RFC 3339 strings sort lexicographically in chronological order.

**However, there are subtle pitfalls:**

1. **User-supplied `since`/`until` parameters may use different formats.** If a client sends `since=2026-02-01T00:00:00Z` (with `Z`) but stored timestamps use `+00:00`, the string comparison still works because `Z` (0x5A) sorts before `+` (0x2B)... wait, no: `+` is 0x2B and `Z` is 0x5A, so `Z` sorts AFTER `+00:00`. This means `2026-02-01T00:00:00Z` > `2026-02-01T00:00:00+00:00` in BINARY collation, even though they represent the same instant. A query with `timestamp >= '2026-02-16T10:30:00Z'` would MISS the row `2026-02-16T10:30:00+00:00` even though they are the same time.
2. **Mixing `T` separator vs space.** Some clients send `2026-02-01 00:00:00` (space separator) vs `2026-02-01T00:00:00` (T separator). Space (0x20) sorts before `T` (0x54), causing incorrect range boundaries.
3. **Date-only parameters.** A client sending `since=2026-02-01` expects "all of February 1st and after" but the stored timestamps all start with `2026-02-01T...` which sorts after `2026-02-01` in BINARY collation, so this actually works. But `until=2026-02-01` would exclude ALL of February 1st because `2026-02-01` < `2026-02-01T00:00:00+00:00`.

**Why it happens:** RFC 3339 and ISO 8601 allow multiple representations of the same instant. TEXT-based comparison is format-sensitive. Chrono's `to_rfc3339()` always uses `+00:00` (not `Z`) for UTC, but user input may use either.

**Consequences:**
- Queries that filter by time silently include or exclude rows at the boundary
- Off-by-one-day errors when using date-only parameters
- Results differ depending on whether the user sends `Z` or `+00:00`

**Prevention:**
1. **Normalize all user-supplied timestamps before using in queries.** Parse with `chrono::DateTime::parse_from_rfc3339()` or `chrono::NaiveDateTime::parse_from_str()`, then re-format with `.to_rfc3339()` to match the stored format. This ensures `Z` -> `+00:00` normalization.
   ```rust
   fn normalize_timestamp(input: &str) -> Result<String, Error> {
       // Try RFC 3339 first (handles both Z and +00:00)
       if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input) {
           return Ok(dt.with_timezone(&chrono::Utc).to_rfc3339());
       }
       // Try date-only: "2026-02-01" -> "2026-02-01T00:00:00+00:00"
       if let Ok(date) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
           let dt = date.and_hms_opt(0, 0, 0).unwrap();
           return Ok(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(dt, chrono::Utc).to_rfc3339());
       }
       Err(Error::BadRequest(format!("Invalid timestamp: {}", input)))
   }
   ```
2. **Document accepted timestamp formats** in the API: RFC 3339 (`2026-02-01T00:00:00Z` or `2026-02-01T00:00:00+00:00`) and date-only (`2026-02-01`).
3. **Add validation tests** for each format variant against actual stored data.
4. **Consider standardizing stored timestamps** to use `to_rfc3339_opts(SecondsFormat::Micros, true)` which uses `Z` instead of `+00:00` and fixed 6-digit microseconds. This would require a migration to reformat existing data but makes TEXT comparisons more predictable. However, this is a "nice to have" -- normalization at query time is sufficient.

**Phase to address:** Query parameter parsing phase (before implementing any time-filtered endpoint). Every endpoint that accepts time parameters must normalize inputs.

**Confidence:** HIGH -- chrono's `to_rfc3339()` output format using `+00:00` is verified in the codebase (all timestamps created at `handlers.rs:170,186,256,333,389,432`). The BINARY collation ordering of `Z` vs `+00:00` is deterministic.

---

## Moderate Pitfalls

Mistakes that cause incorrect results, poor performance, or confusing API behavior.

---

### Pitfall 6: Missing Composite Indexes for GROUP BY Queries

**What goes wrong:** The existing schema has `idx_requests_timestamp ON requests(timestamp)` and `idx_requests_correlation_id ON requests(correlation_id)`. Analytics queries will commonly:

- Group by model: `SELECT model, TOTAL(cost_sats) FROM requests GROUP BY model`
- Filter by time + group by model: `SELECT model, TOTAL(cost_sats) FROM requests WHERE timestamp >= ? GROUP BY model`
- Filter by time + group by provider: `SELECT provider, COUNT(*) FROM requests WHERE timestamp >= ? GROUP BY provider`
- Filter by model + time range: `SELECT ... FROM requests WHERE model = ? AND timestamp BETWEEN ? AND ?`

The single-column `idx_requests_timestamp` helps with time range filters but does not help with `GROUP BY model` (requires a full scan of matching rows to extract model values) or with combined `WHERE model = ? AND timestamp >= ?` (can use the timestamp index for the range but then must scan for model, or vice versa).

**Why it happens:** The schema was designed for write-heavy workloads (INSERT and UPDATE by correlation_id). Analytics is a different access pattern that benefits from different indexes.

**Consequences:**
- `GROUP BY model` queries become slow as the table grows (full table scan to extract model values)
- `WHERE model = ? AND timestamp >= ?` cannot use a covering index, requiring two passes or a scan
- EXPLAIN QUERY PLAN shows "SCAN" instead of "SEARCH" for analytics queries

**Prevention:**
1. Add a composite index for the most common analytics query pattern:
   ```sql
   CREATE INDEX IF NOT EXISTS idx_requests_model_timestamp ON requests(model, timestamp);
   ```
   This covers both `WHERE model = ?` (uses the first column), `WHERE model = ? AND timestamp >= ?` (uses both columns in order), and `GROUP BY model` with time filter (the index already groups by model).
2. Add an index on `provider` for provider-grouped queries:
   ```sql
   CREATE INDEX IF NOT EXISTS idx_requests_provider_timestamp ON requests(provider, timestamp);
   ```
3. Add an index on `success` for filtering failed/successful requests:
   ```sql
   CREATE INDEX IF NOT EXISTS idx_requests_success ON requests(success);
   ```
4. **Do NOT add these indexes preemptively without testing.** Each index slows down INSERTs. For a small table (<100K rows), the full scan is fast enough. Add indexes when EXPLAIN QUERY PLAN shows "SCAN TABLE requests" on queries that are measurably slow.
5. Use `EXPLAIN QUERY PLAN` during development to verify index usage:
   ```sql
   EXPLAIN QUERY PLAN SELECT model, TOTAL(cost_sats) FROM requests WHERE timestamp >= '2026-02-01' GROUP BY model;
   ```

**Phase to address:** Migration phase (ideally before analytics endpoints ship, but can be added later without API changes). Test with representative data volume (>10K rows) before deciding which indexes to add.

**Confidence:** HIGH -- the existing index list is visible in `migrations/20260203000000_initial_schema.sql:20-21`. The absence of composite indexes is factual.

---

### Pitfall 7: COUNT(*) vs COUNT(column) Confusion in Request Counts

**What goes wrong:** The `requests` table has many nullable columns. The distinction between `COUNT(*)` and `COUNT(column)` is critical:

| Expression | Returns |
|---|---|
| `COUNT(*)` | Total rows (including failed requests with NULL tokens) |
| `COUNT(input_tokens)` | Only rows where input_tokens is NOT NULL |
| `COUNT(cost_sats)` | Only rows with non-NULL cost |
| `COUNT(DISTINCT model)` | Number of unique models |

A common mistake: returning `COUNT(*)` as "total requests" alongside `AVG(input_tokens)` as "average input tokens." The consumer assumes the average is over all requests, but it is actually over only the non-NULL subset (see Pitfall 2). If 50% of requests are streaming-without-usage (NULL tokens), the "total" and the "average denominator" are different numbers with no indication in the API response.

A subtler mistake: using `COUNT(provider)` to count "requests served" -- but `provider` is NULL when no provider was selected (routing error). `COUNT(provider)` silently excludes routing failures from the count.

**Why it happens:** `COUNT(*)` and `COUNT(x)` look similar and developers use them interchangeably. The semantic difference (all rows vs non-NULL rows) only manifests when NULLs exist.

**Consequences:**
- "Success rate" computed as `COUNT(provider) / COUNT(*)` gives wrong results because NULL providers (routing errors) are excluded from the numerator but included in the denominator
- Dashboard shows "300 requests, average cost 15 sats" when only 100 of those 300 had costs, making the true average-per-request 5 sats

**Prevention:**
1. Be explicit in every query about what is being counted. Use `COUNT(*)` for total rows and `SUM(CASE WHEN success = 1 THEN 1 ELSE 0 END)` for successful requests rather than `COUNT(provider)`.
2. In the API response, distinguish clearly:
   ```json
   {
     "total_requests": 300,
     "successful_requests": 250,
     "requests_with_cost_data": 100,
     "total_cost_sats": 1500.00,
     "avg_cost_per_billed_request": 15.00
   }
   ```
3. Add code review rules: every `COUNT()` call in an analytics query should have a comment explaining whether it counts all rows or non-NULL rows.

**Phase to address:** API design phase -- the response schema must be designed with NULL-awareness from the start.

**Confidence:** HIGH -- the nullable columns are visible in the schema (`input_tokens INTEGER`, `output_tokens INTEGER`, `cost_sats REAL`, `provider TEXT` -- all without NOT NULL). The `COUNT()` behavior is [standard SQL](https://sqlite.org/lang_aggfunc.html).

---

### Pitfall 8: Query Parameter Validation Gaps Allow SQL Injection via ORDER BY

**What goes wrong:** Axum's `Query<T>` extractor with serde handles type validation for fields like `since: Option<String>` and `limit: Option<u32>`. But analytics endpoints often want to accept `sort_by` or `group_by` parameters that specify column names. If these are interpolated into SQL as strings, they enable SQL injection:

```rust
// DANGEROUS: user-controlled column name in ORDER BY
let query = format!("SELECT * FROM requests ORDER BY {}", params.sort_by);
sqlx::query(&query).fetch_all(pool).await?;
```

Even parameterized queries with `sqlx::query("SELECT * FROM requests ORDER BY ?").bind(sort_by)` do NOT help because SQLite treats `?` as a value placeholder, not an identifier. `ORDER BY ?` orders by the literal string value, not the column name.

**Why it happens:** SQL parameterization protects values but not identifiers (column names, table names). Developers assume `?` binding handles all injection vectors.

**Consequences:**
- SQL injection via `sort_by=1; DROP TABLE requests--`
- Information disclosure via `sort_by=error_message` exposing error details the API was not meant to expose

**Prevention:**
1. **Whitelist allowed column names** using an enum, not string matching:
   ```rust
   #[derive(Deserialize)]
   #[serde(rename_all = "snake_case")]
   enum SortColumn {
       Timestamp,
       Model,
       Provider,
       CostSats,
       LatencyMs,
   }

   impl SortColumn {
       fn as_sql(&self) -> &'static str {
           match self {
               Self::Timestamp => "timestamp",
               Self::Model => "model",
               Self::Provider => "provider",
               Self::CostSats => "cost_sats",
               Self::LatencyMs => "latency_ms",
           }
       }
   }
   ```
   Serde deserialization rejects any value not in the enum. The `as_sql()` method returns a hardcoded `&'static str`, eliminating injection.
2. **Never use `format!()` to build SQL with user input.** Even for identifiers, use match/enum patterns.
3. **Use `LIMIT` with a maximum cap:** `params.limit.unwrap_or(100).min(1000)`. Never allow unbounded queries.

**Phase to address:** Implementation phase -- every analytics handler must use the whitelist pattern from day one.

**Confidence:** HIGH -- this is a well-known SQL injection pattern. The risk is real because analytics endpoints naturally want sort/group parameters.

---

### Pitfall 9: Inconsistent JSON Response Structure Between Analytics and Proxy Endpoints

**What goes wrong:** The existing proxy endpoints return OpenAI-compatible JSON with `error.message`, `error.type`, `error.code` structure for errors (see `error.rs:50-57`). The new analytics endpoints might return errors in a different format (e.g., `{"error": "Invalid parameter"}` without the nested `type` and `code` fields). This creates two error formats that clients must handle differently depending on the URL path.

Similarly, the existing endpoints use different response shapes:
- `/health` returns `{"status": "ok", "service": "arbstr"}`
- `/providers` returns `{"providers": [...]}`
- `/v1/models` returns `{"object": "list", "data": [...]}`

The analytics endpoints need a consistent convention: are they arbstr-extension format (like `/providers`) or OpenAI-compatible (like `/v1/models`)?

**Why it happens:** Different endpoints were added at different times by different design decisions. Without an explicit convention for "arbstr extension" response format, each endpoint reinvents the structure.

**Consequences:**
- Client libraries need different error handling per endpoint
- API documentation is inconsistent
- Adding a generic API client wrapper becomes impossible

**Prevention:**
1. **Reuse the existing `Error` enum and `IntoResponse` impl** for all analytics errors. This guarantees the same `{"error": {"message": ..., "type": "arbstr_error", "code": ...}}` structure.
2. **Define a standard analytics response wrapper:**
   ```rust
   #[derive(Serialize)]
   struct AnalyticsResponse<T: Serialize> {
       data: T,
       #[serde(skip_serializing_if = "Option::is_none")]
       meta: Option<QueryMeta>,
   }

   #[derive(Serialize)]
   struct QueryMeta {
       total_rows: u64,
       since: Option<String>,
       until: Option<String>,
   }
   ```
3. **Place analytics endpoints under a consistent prefix** like `/stats/` or `/analytics/` to clearly separate them from the OpenAI-compatible `/v1/` namespace. Do not put analytics under `/v1/` -- it creates the false impression they are OpenAI-compatible.
4. **Return HTTP 400 for bad query parameters** (not 422 or 500). Use `Error::BadRequest(...)` which already maps to 400 with the standard error format.

**Phase to address:** API design phase -- the response convention must be decided before implementing any endpoint.

**Confidence:** HIGH -- the existing response formats are directly observable in `handlers.rs` and `error.rs`.

---

### Pitfall 10: Not Handling the Database-Disabled Case

**What goes wrong:** The `AppState.db` field is `Option<SqlitePool>` (server.rs:30). When the database fails to initialize, it is `None` and the proxy continues operating without logging (server.rs:97-101). The analytics endpoints must handle this case. If they blindly unwrap `state.db.as_ref().unwrap()`, the proxy panics when the database is disabled.

**Why it happens:** The proxy was designed to work without a database (degraded mode). Analytics endpoints implicitly require a database. If the handler does not check for `None`, the proxy crashes when a user hits an analytics endpoint in degraded mode.

**Consequences:**
- Panic crash when analytics endpoint is called without database
- Entire proxy goes down because of an analytics request

**Prevention:**
1. Check for `state.db` at the start of every analytics handler:
   ```rust
   let pool = state.db.as_ref().ok_or(Error::Internal(
       "Database not available -- analytics disabled".to_string()
   ))?;
   ```
2. Alternatively, only register analytics routes when the database is available:
   ```rust
   let mut app = Router::new()
       .route("/v1/chat/completions", post(handlers::chat_completions))
       // ...always-available routes...
       ;
   if state.db.is_some() {
       app = app
           .route("/stats/summary", get(handlers::stats_summary))
           .route("/stats/costs", get(handlers::stats_costs));
   }
   ```
   This returns 404 instead of 500 when analytics are unavailable, which is more informative.
3. Whichever approach, add a test for the no-database case.

**Phase to address:** Handler scaffolding phase -- the `Option<SqlitePool>` check must be the first thing each handler does.

**Confidence:** HIGH -- `AppState.db` is `Option<SqlitePool>` at `server.rs:30`, and the `None` case is explicitly handled for write paths but no read paths exist yet.

---

## Minor Pitfalls

Issues that cause inconvenience or suboptimal behavior but are easily fixed.

---

### Pitfall 11: Large Result Sets Without Pagination

**What goes wrong:** An endpoint like `GET /stats/requests` that returns individual request logs can return millions of rows as the database grows. Without `LIMIT` and `OFFSET` (or cursor-based pagination), the response is enormous, the query takes seconds, and the client may time out or OOM.

**Prevention:**
1. Default `LIMIT 100` on all list endpoints. Accept `?limit=N` with a maximum cap (e.g., 1000).
2. For aggregate endpoints (summaries, totals), there is no pagination concern -- the result is a single row or a small group.
3. For list endpoints, use cursor-based pagination (e.g., `?after=<id>`) rather than OFFSET-based. OFFSET-based pagination becomes slow for large offsets because SQLite must scan and discard OFFSET rows.
4. Include pagination metadata in responses: `{"data": [...], "has_more": true, "next_cursor": "12345"}`.

**Phase to address:** Implementation phase.

**Confidence:** HIGH -- standard API design concern.

---

### Pitfall 12: Time Zone Assumptions in "Daily" or "Hourly" Aggregation

**What goes wrong:** If analytics endpoints support grouping by time buckets (e.g., `?group_by=day`), the bucketing depends on the time zone. "Today" in UTC is a different set of hours than "today" in US/Pacific. Since all stored timestamps are UTC, grouping by day uses UTC days. A user in UTC-8 sees their 4pm request counted as "tomorrow" in the analytics.

**Prevention:**
1. Document that all time-based grouping uses UTC.
2. Do NOT accept a timezone parameter in v1 -- it adds complexity (SQLite has no native timezone support, requiring `strftime` with offset math).
3. Use SQLite's `strftime` for bucketing:
   ```sql
   SELECT strftime('%Y-%m-%d', timestamp) as day, TOTAL(cost_sats) as daily_cost
   FROM requests
   WHERE timestamp >= ? AND timestamp < ?
   GROUP BY day ORDER BY day;
   ```
   This groups by UTC day, which is deterministic and simple.
4. For future timezone support, accept `tz_offset_hours` as an integer (-12 to +14) and use `strftime('%Y-%m-%d', timestamp, '+N hours')`.

**Phase to address:** Design phase (document UTC-only), defer timezone support to a later milestone.

**Confidence:** MEDIUM -- depends on whether time-bucketed aggregation is in scope for this milestone.

---

### Pitfall 13: Forgetting to Test with NULL-Heavy Data

**What goes wrong:** Unit tests for analytics queries are written with clean seed data where every row has non-NULL tokens and costs. The tests pass. In production, 30-70% of rows have NULL tokens (streaming without usage, failed requests, timeouts). The aggregate functions behave differently with NULLs (see Pitfalls 1, 2, 7) and edge cases emerge.

**Prevention:**
1. Seed test data with realistic NULL distributions:
   ```rust
   // 50% of test rows should have NULL tokens/cost (realistic for streaming-heavy workloads)
   let test_rows = vec![
       row(success=true, tokens=Some(100), cost=Some(5.0)),
       row(success=true, tokens=None, cost=None),          // streaming without usage
       row(success=false, tokens=None, cost=None),          // failed request
       row(success=true, tokens=Some(200), cost=Some(10.0)),
       row(success=true, tokens=None, cost=None),           // streaming without usage
   ];
   ```
2. Test the "all NULL" edge case: a time range where every row has NULL cost. Verify the API returns 0.0 (not null or error).
3. Test the "no rows" edge case: a time range with zero matching rows. Verify the API returns sensible defaults (0 for counts, 0.0 for totals, null or absent for averages).
4. Test the "one row" edge case: verify averages equal the single value.

**Phase to address:** Test design phase -- define the seed data template before writing any test.

**Confidence:** HIGH -- the nullable columns and the streaming-without-usage pattern are well-documented in the codebase (see `RequestLog` struct and `spawn_stream_completion_update` with `None` tokens).

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| Database schema migration (adding indexes) | Adding too many indexes slows INSERTs on the hot path | Benchmark INSERT latency before/after; add only indexes proved necessary by EXPLAIN QUERY PLAN |
| Query parameter parsing | `Z` vs `+00:00` timestamp mismatch (Pitfall 5) | Normalize all timestamps through chrono parsing before SQL |
| Aggregate queries | SUM returning NULL for all-NULL groups (Pitfall 1) | Use TOTAL() instead of SUM() for cost aggregations |
| Aggregate queries | AVG misleading denominator (Pitfall 2) | Always return sample count alongside averages |
| Response format | Inconsistent error/success shapes (Pitfall 9) | Define response wrapper types before implementing handlers |
| Connection management | Analytics queries starving proxy writes (Pitfall 4) | Implement read-only pool before writing any analytics handler |
| Security | SQL injection via sort/group parameters (Pitfall 8) | Enum whitelist for all column name parameters |
| Testing | Clean test data hiding NULL bugs (Pitfall 13) | Require 50%+ NULL rows in analytics test fixtures |
| API design | Unbounded result sets (Pitfall 11) | Default LIMIT with max cap on all list endpoints |

## Integration Pitfalls

Mistakes when connecting analytics endpoints to the existing system.

| Integration Point | Common Mistake | Correct Approach |
|---|---|---|
| `AppState.db: Option<SqlitePool>` | Unwrapping without checking for `None` | Return 503 or exclude analytics routes when DB is disabled |
| Existing `Error` enum | Creating separate error types for analytics | Reuse `Error::BadRequest`, `Error::Internal`, `Error::Database` |
| Existing response headers | Forgetting to set `Content-Type: application/json` | Use `axum::Json()` which sets it automatically |
| Route registration in `create_router()` | Adding analytics routes inside `/v1/` namespace | Use `/stats/` prefix to avoid OpenAI API confusion |
| `chrono` dependency | Pulling in additional time-parsing crates | `chrono` is already in `Cargo.toml` and handles RFC 3339 parsing |
| sqlx query macros | Using `sqlx::query!()` (compile-time checked) in analytics when queries are dynamic | Use `sqlx::query()` (runtime) for analytics with dynamic WHERE clauses; use `sqlx::query_as!()` only for static queries |

## "Looks Done But Isn't" Checklist

- [ ] **TOTAL() not SUM():** Every cost aggregation uses `TOTAL()` or `COALESCE(SUM(), 0.0)`
- [ ] **AVG with sample size:** Every average is accompanied by the count of non-NULL values in the response
- [ ] **Timestamp normalization:** Every `since`/`until` parameter is parsed and re-formatted before SQL
- [ ] **Read-only pool:** Analytics handlers use a separate read-only connection pool
- [ ] **No-database guard:** Every analytics handler checks `state.db.is_some()` before proceeding
- [ ] **Column whitelist:** Any `sort_by` or `group_by` parameter uses an enum, not string interpolation
- [ ] **Response format:** Analytics responses use the same error format as proxy endpoints
- [ ] **LIMIT cap:** List endpoints have a default and maximum LIMIT
- [ ] **Cost rounding:** All cost values in responses are rounded to 2 decimal places
- [ ] **NULL test data:** Test fixtures include rows with NULL tokens, NULL cost, NULL provider
- [ ] **Empty result test:** Tested with zero matching rows -- returns 0/0.0, not null or error
- [ ] **Index verification:** EXPLAIN QUERY PLAN confirms index usage for the most common queries

## Sources

- [SQLite Built-in Aggregate Functions](https://sqlite.org/lang_aggfunc.html) -- SUM vs TOTAL, COUNT(*) vs COUNT(X), AVG NULL-skipping behavior
- [SQLite Floating Point Numbers](https://sqlite.org/floatingpoint.html) -- IEEE 754 precision limitations in aggregates
- [SQLite Forum: Sum precision](https://sqlite.org/forum/info/13a427e233fc15a0) -- cumulative floating-point error in SUM/TOTAL
- [SQLite Forum: Numerical stability of AVG and SUM](https://sqlite.org/forum/info/a0b458d8ef6156a75aa2ed7f5d0391fa877ee7609329e7ef86ae2570f79442cf) -- Kahan summation discussion
- [SQLite Write-Ahead Logging](https://sqlite.org/wal.html) -- WAL checkpoint blocking by long-running readers
- [SQLite Query Planner](https://sqlite.org/queryplanner.html) -- index selection for range queries on TEXT columns
- [SQLite EXPLAIN QUERY PLAN](https://sqlite.org/eqp.html) -- verifying index usage
- [SQLite SUM vs TOTAL: What's the Difference?](https://database.guide/sqlite-sum-vs-total-whats-the-difference/) -- practical comparison
- [SQLite performance tuning](https://phiresky.github.io/blog/2020/sqlite-performance-tuning/) -- concurrent read optimization, WAL file growth
- [Battling with SQLite in a Concurrent Environment](https://www.drmhse.com/posts/battling-with-sqlite-in-a-concurrent-environment/) -- connection pool contention with read/write mixed workloads
- [axum Query Extractor](https://docs.rs/axum/latest/axum/extract/struct.Query.html) -- serde deserialization for query parameters
- [chrono SecondsFormat](https://docs.rs/chrono/latest/chrono/format/enum.SecondsFormat.html) -- variable-length subsecond formatting in to_rfc3339()
- [The COUNT(X) function only counts non-null values](https://alexwlchan.net/til/2024/count-only-counts-non-null-values/) -- practical demonstration of COUNT gotcha
- Direct codebase analysis: `storage/logging.rs` (RequestLog struct, nullable fields), `storage/mod.rs` (pool configuration), `handlers.rs` (timestamp formatting, response patterns), `error.rs` (error response format), `server.rs` (AppState with Option<SqlitePool>), `migrations/*.sql` (schema and existing indexes)

---
*Pitfalls research for: cost querying API / analytics endpoints on SQLite-backed proxy*
*Researched: 2026-02-16*
