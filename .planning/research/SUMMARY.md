# Project Research Summary

**Project:** arbstr - Cost Query API Endpoints
**Domain:** Read-only analytics/stats API for SQLite-backed LLM routing proxy
**Researched:** 2026-02-16
**Confidence:** HIGH

## Executive Summary

This milestone adds read-only HTTP endpoints to expose aggregate cost and usage statistics from arbstr's existing SQLite request logs. Research shows this is a well-established pattern across LLM proxies (OpenAI Usage API, LiteLLM, Helicone, Portkey) and API gateways. The core approach is straightforward: aggregate SQL queries (COUNT, SUM, AVG, GROUP BY) against the existing `requests` table, exposed via axum GET endpoints with time-range filtering and model/provider breakdown.

The recommended implementation requires zero new dependencies. The existing stack (axum 0.7, sqlx 0.8, chrono 0.4, serde) provides every capability needed: axum's `Query` extractor for query parameters, sqlx's `query_as()` for aggregates with typed results, chrono for RFC3339 timestamp parsing, and serde for JSON responses. The architecture separates data access (`storage/stats.rs` with pure query functions) from HTTP concerns (`proxy/handlers.rs` with parameter validation and response formatting), following the existing codebase pattern.

The primary risks are SQL-specific pitfalls around NULL handling and floating-point precision. SQLite's `SUM(cost_sats)` returns NULL when all rows have NULL costs (streaming requests without usage data), which propagates silently through JSON responses. Use `TOTAL()` instead of `SUM()` to return 0.0 for all-NULL groups. Additionally, long-running analytics queries can starve the proxy's write path by monopolizing the shared connection pool. Prevention requires a separate read-only SQLite pool for analytics. These pitfalls are well-documented and addressable with targeted mitigations in the first implementation phase.

## Key Findings

### Recommended Stack

No new dependencies required. The existing stack fully supports read-only analytics endpoints. Research across sqlx documentation, axum Query extractor patterns, and SQLite aggregate behavior confirms every capability is already present.

**Core technologies:**
- **axum 0.7**: HTTP server with built-in `Query<T>` extractor for deserializing URL parameters into typed structs. Route registration via `Router::nest()` for `/v1/arbstr/stats/*` endpoints.
- **sqlx 0.8**: `query_as()` runtime function with `#[derive(FromRow)]` for mapping aggregate SELECT results to typed structs. `query_scalar()` for single-value aggregates. Already configured with SQLite in WAL mode for concurrent read/write.
- **chrono 0.4**: RFC3339 timestamp parsing (`parse_from_rfc3339()`) for validating and normalizing `since`/`until` query parameters. Already used for timestamp generation throughout the codebase.
- **serde + serde_json**: Deserialize query params into structs with `#[derive(Deserialize)]`. Serialize response types with `#[derive(Serialize)]`. `axum::Json<T>` handles automatic JSON response wrapping.

**Critical finding:** Zero new crates. This milestone uses only existing dependencies. The existing `idx_requests_timestamp` index supports time-range filtering efficiently. Composite indexes (model, provider) can be added later if GROUP BY queries prove slow with large datasets.

### Expected Features

Based on patterns from OpenAI Usage API, LiteLLM spend tracking, Helicone cost analytics, and general API gateway monitoring, the feature landscape divides into table stakes (expected from any analytics API) and differentiators (power-user features).

**Must have (table stakes):**
- **Aggregate summary endpoint** — total requests, total cost, total tokens, avg latency, success rate. Universal across LLM proxies and API gateways.
- **Time range filtering** — `?start=...&end=...` with ISO 8601 timestamps. Every analytics API supports scoped queries (last hour, today, this month).
- **Preset time ranges** — `?range=last_24h` shortcuts to avoid epoch math. Common across LiteLLM, Helicone, GitBook analytics.
- **Per-model breakdown** — GROUP BY model with same aggregate fields. Users need to know which models cost the most.
- **Per-provider breakdown** — GROUP BY provider. Core value proposition for arbstr as a multi-provider proxy.
- **Success rate** — `COUNT(success=true) / COUNT(*)`. Standard monitoring metric, essential for multi-provider reliability tracking.

**Should have (competitive):**
- **Time-series bucketed data** — GROUP BY hour/day for trend analysis and dashboards. OpenAI Usage API supports `bucket_width`, Prometheus uses `step`. Medium complexity (SQLite `strftime` grouping).
- **Request log listing with pagination** — Browse individual requests for debugging. Low complexity, high utility. LiteLLM exposes `/spend/logs`.
- **Latency percentiles (p50/p95/p99)** — Standard in API gateway monitoring. SQLite lacks native percentile functions, requires in-app computation.

**Defer (v2+):**
- **GraphQL analytics API** — Overkill for a single-user local proxy. Predictable query patterns fit REST endpoints.
- **Real-time streaming (WebSocket/SSE)** — Marginal value for local tool. Clients can poll.
- **Export to CSV/Prometheus/OpenTelemetry** — SQLite database is directly accessible. Premature integration.
- **Budget alerts** — Budget enforcement belongs in policy engine (separate milestone), not query endpoints.

### Architecture Approach

The recommended architecture separates data access from HTTP concerns, following the existing codebase pattern. All SQL queries live in a new `src/storage/stats.rs` module alongside `logging.rs` (write operations). Query functions take `&SqlitePool` and return `Result<T, sqlx::Error>` with typed response structs. Handlers in `src/proxy/handlers.rs` extract query parameters via `axum::Query<T>`, validate time ranges, call storage query functions, and wrap results in `Json<T>` responses.

**Major components:**
1. **`storage/stats.rs`** — Pure data access layer with query functions (`get_summary`, `get_cost_by_model`, `get_cost_by_provider`, `get_recent_requests`) and response types (`Summary`, `ModelStats`, `ProviderStats`, `RequestEntry`). No HTTP dependency, independently testable with in-memory SQLite.
2. **`proxy/handlers.rs`** — Stats handler functions with query parameter structs (`TimeRangeParams`, `RecentParams`) and time range validation helper (`resolve_time_range`). Extracts `State<AppState>` and `Query<Params>`, validates inputs, calls storage functions, returns `Json<Response>`.
3. **`proxy/server.rs`** — Route registration via `Router::nest("/v1/arbstr/stats", stats_routes)` with nested routes for `/summary`, `/models`, `/providers`, `/requests`. All routes share `AppState` and middleware.

**Key patterns:**
- Use `TOTAL()` instead of `SUM()` for cost aggregations (returns 0.0 for all-NULL groups, not NULL).
- Normalize user timestamps through `chrono::parse_from_rfc3339()` before SQL to handle `Z` vs `+00:00` format variations.
- Whitelist column names for `sort_by`/`group_by` parameters using enums (not string interpolation) to prevent SQL injection.
- Create a separate read-only `SqlitePool` for analytics to avoid starving proxy writes.

### Critical Pitfalls

Research identified 13 pitfalls across critical, moderate, and minor severity. The top 5 require explicit mitigation in the first implementation phase.

1. **SUM(cost_sats) returns NULL when all rows have NULL cost** — Streaming requests without usage data have `cost_sats = NULL`. SQLite's `SUM()` returns NULL for all-NULL groups, propagating through JSON responses. Prevention: Use `TOTAL()` function (returns 0.0 instead of NULL) or `COALESCE(SUM(x), 0.0)`. Address in first phase — every aggregate query must use the correct function from the start.

2. **Analytics queries starve proxy writes via SQLite locking** — The shared 5-connection pool serves both reads (analytics) and writes (proxy logs). Long-running analytics queries monopolize connections, prevent WAL checkpointing, and cause proxy write timeouts. Prevention: Create a separate read-only `SqlitePool` with 2 connections for analytics. Read-only connections in WAL mode never conflict with writes. Address in infrastructure phase before implementing endpoints.

3. **Timestamp range queries fail with inconsistent RFC3339 formatting** — User input may use `Z` vs `+00:00` for UTC. BINARY collation treats `Z` (0x5A) and `+` (0x2B) as different characters, causing incorrect range boundaries. Stored timestamps use `+00:00` (from `chrono::to_rfc3339()`). Prevention: Normalize all user timestamps through `parse_from_rfc3339()` and re-format before SQL. Address in query parameter parsing phase.

4. **AVG(cost_sats) excludes NULL rows silently** — `AVG()` only considers non-NULL values. If 80% of requests have NULL cost, `AVG()` computes average of the 20% that have costs without indicating coverage. Prevention: Return both `AVG(cost_sats)` and `COUNT(cost_sats)` alongside `COUNT(*)` so consumers know sample size. Address in API design phase.

5. **SQL injection via ORDER BY with dynamic column names** — `sort_by` parameters cannot use `?` binding (SQLite treats `?` as value, not identifier). String interpolation enables injection. Prevention: Whitelist column names using serde enum with `as_sql()` method returning `&'static str`. Reject any value not in enum. Address in implementation phase.

**Additional moderate pitfalls:** Missing composite indexes for GROUP BY (add when measurably slow), COUNT(*) vs COUNT(column) confusion (be explicit about NULL-aware vs total counts), inconsistent JSON response structure (reuse existing `Error` enum), not handling database-disabled case (check `state.db.is_some()` in every handler).

## Implications for Roadmap

Based on research, this milestone naturally divides into 3 phases with clear dependency ordering. The foundation must establish infrastructure (read pool, response conventions) before implementing table-stakes features, then extend to competitive differentiators.

### Phase 1: Infrastructure and Core Aggregates
**Rationale:** The read-only connection pool and time range validation are prerequisites for any analytics endpoint. The aggregate summary endpoint is the foundation — time filtering, per-model breakdown, and per-provider breakdown are parameters on this endpoint, not separate endpoints. Build the core before specializing.

**Delivers:**
- Separate read-only SQLite pool for analytics (prevents proxy write starvation)
- Time range parameter parsing with RFC3339 normalization
- `GET /v1/arbstr/stats/summary` endpoint with aggregate fields (total requests, total cost, total tokens, avg latency, success rate)
- Per-model breakdown (`?group_by=model`)
- Per-provider breakdown (`?group_by=provider`)

**Addresses:**
- Must-have features: aggregate summary, time range filtering, per-model breakdown, per-provider breakdown, success rate
- Preset time ranges: `?range=last_24h|last_7d|last_30d` as shortcuts

**Avoids:**
- Pitfall 4 (analytics queries starving proxy) — separate read pool implemented first
- Pitfall 5 (timestamp format inconsistency) — normalization helper before first endpoint
- Pitfall 1 (SUM NULL) — use `TOTAL()` from the start
- Pitfall 10 (database-disabled case) — check `state.db.is_some()` in handlers

**Technical notes:**
- New files: `src/storage/stats.rs` (query functions), route registration in `server.rs`
- Modified files: `src/storage/mod.rs` (read pool init, re-exports), `src/proxy/handlers.rs` (stats handlers)
- No new dependencies, no schema changes (existing indexes sufficient)

### Phase 2: Request Log Listing
**Rationale:** Log listing is a separate access pattern (individual rows, not aggregates) with different concerns (pagination, sorting). Should be implemented after aggregates are proven working. Low complexity but requires pagination design and cursor/offset strategy.

**Delivers:**
- `GET /v1/arbstr/stats/requests` endpoint with pagination
- Filtering by model, provider, success, streaming
- Sorting by timestamp, cost, latency
- Pagination metadata (page, per_page, total, has_more)

**Addresses:**
- Should-have feature: request log listing with pagination
- Top-N expensive requests (special case: `?sort=cost_desc&per_page=10`)

**Avoids:**
- Pitfall 11 (unbounded result sets) — default LIMIT 100 with max cap 1000
- Pitfall 8 (SQL injection) — enum whitelist for sort_by parameter

**Technical notes:**
- Uses same read pool and time range helpers from Phase 1
- `RequestEntry` struct with `#[derive(sqlx::FromRow)]` for direct row mapping
- Cursor-based pagination (using `after=<id>`) preferred over OFFSET for large datasets

### Phase 3: Time-Series Bucketing (Differentiator)
**Rationale:** Time-series data is only valuable once someone builds a dashboard or monitoring integration. Medium complexity (SQLite `strftime` for grouping by hour/day) and not required for basic cost visibility. Defer to validate demand first.

**Delivers:**
- `GET /v1/arbstr/stats/timeseries` endpoint
- Bucketing by hour (`?bucket=1h`) or day (`?bucket=1d`)
- Same aggregate fields as summary, grouped by time bucket
- Array response with `{start, end, total_cost_sats, avg_latency_ms, ...}` per bucket

**Addresses:**
- Should-have feature: time-series bucketed data for trend analysis

**Avoids:**
- Pitfall 12 (timezone assumptions) — document UTC-only bucketing, defer timezone support

**Technical notes:**
- SQL: `GROUP BY strftime('%Y-%m-%d %H:00:00', timestamp)` for hourly buckets
- Response includes explicit `bucket` field and `period.start/end` for clarity
- Can reuse aggregate field calculations from Phase 1

### Phase Ordering Rationale

- **Infrastructure first:** The read-only connection pool (Phase 1) prevents analytics queries from degrading proxy performance. Must be in place before any endpoint ships. Time range normalization (Phase 1) prevents silent bugs from format mismatches. These are non-negotiable prerequisites.

- **Aggregates before details:** The summary endpoint (Phase 1) answers the primary user question: "How much have I spent?" Log listing (Phase 2) is a debugging/audit feature that depends on the same filtering and time range logic. Build the main use case first.

- **Differentiation after validation:** Time-series bucketing (Phase 3) is only valuable with a dashboard consumer. Shipping Phase 1 and Phase 2 provides complete cost visibility. Phase 3 can be deferred or skipped if demand does not materialize.

- **Dependency ordering:** Phase 2 and 3 both depend on Phase 1's infrastructure (read pool, time helpers). Phase 2 and 3 are independent of each other and could be swapped or parallelized.

### Research Flags

Phases with standard patterns (skip research-phase):
- **Phase 1-3:** All phases use well-documented patterns (axum Query extractor, sqlx aggregate queries, SQLite text timestamp comparison). No niche domains or sparse documentation. The research files provide implementation-ready guidance with SQL examples, Rust patterns, and pitfall checklists.

**No additional phase research needed.** This project research is comprehensive and implementation-ready. The STACK.md provides zero-dependency confirmation, FEATURES.md includes endpoint design with query parameters and response shapes, ARCHITECTURE.md specifies file structure and data flow, and PITFALLS.md includes prevention strategies with code examples.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Verified against existing Cargo.toml, sqlx/axum/chrono docs, and codebase patterns. Zero new dependencies confirmed. |
| Features | HIGH | Cross-referenced OpenAI Usage API, LiteLLM, Helicone, Portkey, and API gateway patterns. Table stakes vs differentiators validated across multiple sources. |
| Architecture | HIGH | Based on direct codebase analysis (existing handlers, AppState, storage module structure). Patterns match existing write-path implementation. |
| Pitfalls | HIGH | Sourced from SQLite official docs (SUM/TOTAL, WAL concurrency), sqlx aggregate issues (GitHub, docs.rs), and chrono RFC3339 formatting behavior. All critical pitfalls have verified prevention strategies. |

**Overall confidence:** HIGH

### Gaps to Address

No critical gaps. All areas are implementation-ready. Minor considerations for future phases:

- **Latency percentiles (deferred):** SQLite lacks native PERCENTILE_CONT. If needed later, compute in-app by fetching sorted latency values and picking indices. Performance concern at scale — requires loading all latency values into memory. Not essential for v1.

- **Cost savings estimate (deferred):** Requires cross-referencing request data with provider config rates to compute "max cost - actual cost" savings. Compelling metric but not essential for basic cost visibility. Can be added after Phase 1-3 ship.

- **Composite indexes (conditional):** Research recommends NOT adding `idx_requests_model_timestamp` or `idx_requests_provider_timestamp` preemptively. The existing `idx_requests_timestamp` index covers time-range filtering. Add composite indexes only after EXPLAIN QUERY PLAN shows table scans on measurably slow queries. Each index slows INSERT performance.

- **Bucketing timezones (deferred):** Phase 3 time-series bucketing uses UTC-only. Timezone support requires `strftime('%Y-%m-%d', timestamp, '+N hours')` offset math and adds API complexity. Document UTC-only in v1, add timezone parameter in future if requested.

## Sources

### Primary (HIGH confidence)
- Existing codebase: `Cargo.toml`, `src/proxy/handlers.rs`, `src/proxy/server.rs`, `src/storage/logging.rs`, `src/storage/mod.rs`, `src/error.rs`, `migrations/*.sql` — verified via direct file inspection
- [SQLite Aggregate Functions Reference](https://sqlite.org/lang_aggfunc.html) — SUM vs TOTAL, COUNT(*) vs COUNT(column), AVG NULL-skipping
- [SQLite Write-Ahead Logging](https://sqlite.org/wal.html) — WAL concurrency, checkpoint blocking, read/write isolation
- [SQLite Floating Point Numbers](https://sqlite.org/floatingpoint.html) — IEEE 754 precision limitations
- [sqlx query_as documentation](https://docs.rs/sqlx/latest/sqlx/fn.query_as.html) — runtime query_as with FromRow
- [axum Query extractor](https://docs.rs/axum/latest/axum/extract/struct.Query.html) — query string deserialization patterns
- [chrono to_rfc3339](https://docs.rs/chrono/latest/chrono/struct.DateTime.html#method.to_rfc3339) — output format (+00:00 vs Z)
- [OpenAI Usage API Reference](https://platform.openai.com/docs/api-reference/usage) — bucket_width, group_by patterns
- [OpenAI Costs API Reference](https://developers.openai.com/api/reference/resources/organization/subresources/audit_logs/methods/get_costs) — cost tracking endpoint design

### Secondary (MEDIUM confidence)
- [LiteLLM Spend Tracking](https://docs.litellm.ai/docs/proxy/cost_tracking) — per-model/provider breakdown patterns
- [Helicone Cost Tracking](https://docs.helicone.ai/guides/cookbooks/cost-tracking) — analytics patterns for LLM proxies
- [Portkey Cost Management](https://portkey.ai/docs/product/observability/cost-management) — filtering by provider/model/timeframe
- [SQLite Forum: Numerical stability of SUM](https://sqlite.org/forum/info/a0b458d8e) — Kahan summation discussion
- [Battling with SQLite in a Concurrent Environment](https://www.drmhse.com/posts/battling-with-sqlite-in-a-concurrent-environment/) — connection pool contention patterns
- [Prometheus Query Range API](https://prometheus.io/docs/prometheus/latest/querying/basics/) — time-series bucketing with start/end/step
- [GitBook Events Aggregation API](https://gitbook.com/docs/guides/docs-analytics/track-advanced-analytics-with-gitbooks-events-aggregation-api) — preset time range enums

---
*Research completed: 2026-02-16*
*Ready for roadmap: yes*
