# Feature Landscape: Cost Querying API Endpoints

**Domain:** Read-only analytics/cost query endpoints for an LLM routing proxy
**Researched:** 2026-02-16
**Overall confidence:** HIGH (well-established patterns across OpenAI Usage API, LiteLLM, Helicone, Portkey, and general API gateway analytics)

## Current State Summary

arbstr already logs every request to SQLite with: model, provider, policy, streaming (bool), input_tokens, output_tokens, cost_sats (f64), provider_cost_sats, latency_ms, stream_duration_ms, success (bool), error_status, error_message, and timestamp. The `requests` table has indexes on `correlation_id` and `timestamp`. However, there are **zero query endpoints** -- the only way to see this data is via direct SQLite access. This milestone adds read-only HTTP endpoints to expose aggregated and filtered views of the logged data.

---

## Table Stakes

Features every analytics API in this domain provides. Missing any of these makes the analytics endpoints feel incomplete or unusable.

| Feature | Why Expected | Complexity | Depends On | Notes |
|---------|--------------|------------|------------|-------|
| **Aggregate summary endpoint** | Every LLM proxy (LiteLLM, Helicone, Portkey) and API gateway (Kong, Tyk) exposes a single endpoint returning totals: total_requests, total_cost, total_tokens, avg_latency, success_rate. OpenAI's own `/organization/costs` endpoint follows this pattern. Without it, users cannot answer "how much have I spent?" | Low | Existing `requests` table, SQLite pool in AppState | Single SQL query with `COUNT`, `SUM`, `AVG`. Return JSON. Path: `GET /stats` or `GET /v1/stats`. |
| **Time range filtering** | Universal across all analytics APIs. OpenAI uses `start_time`/`end_time` (unix timestamps). LiteLLM uses `start_date`/`end_date` (ISO date strings). Prometheus uses `start`/`end`/`step`. Users need to scope queries to "last hour", "today", "this month". Without time filtering, the summary is always an all-time total, which becomes less useful over time. | Low | Aggregate summary endpoint | Query params: `start` and `end` as ISO 8601 timestamps (e.g., `2026-02-16T00:00:00Z`). Add `WHERE timestamp >= ? AND timestamp < ?` to queries. arbstr already stores timestamps as ISO 8601 text, and SQLite text comparison works correctly for ISO 8601 ordering. |
| **Preset time range shortcuts** | LiteLLM, Helicone, GitBook, and Adobe Analytics all offer preset ranges alongside custom date ranges. Users want `?range=last_24h` without computing epoch math. Common presets: `last_1h`, `last_24h`, `last_7d`, `last_30d`. | Low | Time range filtering | Resolve presets to `start`/`end` on the server. Accept `range` query param as an alternative to explicit `start`/`end`. If both provided, `start`/`end` takes precedence. |
| **Per-model breakdown** | OpenAI groups by `model` in their Usage API. LiteLLM's `/user/daily/activity` response includes a `breakdown.models` object keyed by model name. Helicone tracks cost-per-model as a core metric. Users need to know which models cost the most. | Low | Aggregate summary endpoint | `GROUP BY model` in SQL. Return an array of objects, each with model name + same aggregate fields (request_count, total_cost, total_input_tokens, total_output_tokens, avg_latency, success_rate). |
| **Per-provider breakdown** | Unique to multi-provider proxies like arbstr. LiteLLM's daily activity includes `breakdown.providers`. Since arbstr's core value is routing across providers, users need to see which providers are cheapest, fastest, and most reliable. | Low | Aggregate summary endpoint | `GROUP BY provider`. Same aggregate fields as per-model breakdown. Provider is nullable in the schema (pre-route errors), so include a null/unknown bucket. |
| **Success rate** | Every monitoring system tracks this. Calculated as `COUNT(success=true) / COUNT(*)`. Users need to know if a provider or model is failing frequently. Particularly important for arbstr where retry/fallback masks failures from the client. | Low | Aggregate summary endpoint | `CAST(SUM(CASE WHEN success THEN 1 ELSE 0 END) AS REAL) / COUNT(*)`. Return as a float 0.0-1.0. |

### Aggregate Response Fields (Table Stakes)

Based on the data available in arbstr's `requests` table and patterns from OpenAI, LiteLLM, and Helicone, the aggregate summary should include:

| Field | Type | SQL | Notes |
|-------|------|-----|-------|
| `total_requests` | integer | `COUNT(*)` | All requests in range |
| `total_cost_sats` | float | `SUM(cost_sats)` | Total spend in satoshis |
| `total_input_tokens` | integer | `SUM(input_tokens)` | May be null if tokens unavailable |
| `total_output_tokens` | integer | `SUM(output_tokens)` | May be null if tokens unavailable |
| `avg_latency_ms` | float | `AVG(latency_ms)` | Mean latency across all requests |
| `avg_cost_sats` | float | `AVG(cost_sats)` | Mean cost per request |
| `success_rate` | float | see above | 0.0 to 1.0 |
| `total_errors` | integer | `SUM(CASE WHEN NOT success ...)` | Count of failed requests |
| `streaming_requests` | integer | `SUM(CASE WHEN streaming ...)` | Streaming vs non-streaming split |

**Confidence:** HIGH -- these fields map directly to columns already in the `requests` table and match patterns from OpenAI Usage API, LiteLLM spend tracking, and Helicone cost analytics.

---

## Differentiators

Features that go beyond the basics. Not expected from a v1 analytics API, but signal maturity and are valued by power users.

| Feature | Value Proposition | Complexity | Depends On | Notes |
|---------|-------------------|------------|------------|-------|
| **Time-series bucketed data** | OpenAI's Usage API supports `bucket_width` of `1m`, `1h`, `1d`. Prometheus uses `step`. This lets users plot cost/usage over time rather than seeing a single aggregate. Essential for dashboards and trend analysis. | Medium | Time range filtering | `GROUP BY strftime('%Y-%m-%d %H:00:00', timestamp)` for hourly buckets. Support `bucket` param: `1h`, `1d`. Return array of `{start_time, end_time, ...aggregate_fields}`. SQLite's `strftime` handles this natively. |
| **Group-by parameter** | OpenAI accepts `group_by=["model","project_id"]`. LiteLLM supports grouping by tags, teams, users. A generic `group_by` param lets one endpoint serve multiple breakdowns: `?group_by=model`, `?group_by=provider`, `?group_by=model,provider`. | Medium | Aggregate summary endpoint | Dynamic SQL construction (allowlisted column names only to prevent injection). Validate that `group_by` values are in `{model, provider, policy, streaming}`. Return nested grouping in response. |
| **Latency percentiles (p50, p95, p99)** | Standard in API gateway monitoring (AWS API Gateway, Datadog, Grafana). `AVG` hides outliers; percentiles reveal tail latency. P95 is the most common SLO target. P99 exposes architectural bottlenecks. | Medium | Aggregate summary endpoint | SQLite does not have native percentile functions. Options: (a) use `PERCENTILE_CONT` via a SQLite extension, (b) compute in Rust by fetching sorted latency values and picking indices, (c) use `NTILE` window function approximation. Option (b) is simplest for SQLite -- fetch `SELECT latency_ms FROM requests WHERE ... ORDER BY latency_ms` and compute percentiles in application code. Performance concern at scale (loading all latency values). |
| **Request log listing with pagination** | Beyond aggregates, let users browse individual request logs. LiteLLM exposes `/spend/logs` with pagination. Useful for debugging specific requests, auditing costs, finding outliers. | Low | Existing data | `SELECT * FROM requests WHERE ... ORDER BY timestamp DESC LIMIT ? OFFSET ?`. Return `{data: [...], total: N, page: N, per_page: N}`. Support filtering by model, provider, success, streaming. |
| **Top-N expensive requests** | Show the N most expensive individual requests. Useful for identifying runaway costs, unexpected large prompts, or misconfigured models. | Low | Request log listing | `SELECT * FROM requests WHERE ... ORDER BY cost_sats DESC LIMIT ?`. A specific sorting preset of the log listing endpoint. |
| **Cost savings estimate** | Since arbstr routes to the cheapest provider, show how much was saved compared to the most expensive provider for the same model. Requires comparing `cost_sats` against what the most expensive provider would have charged. | Medium | Per-model + per-provider breakdown, config access | For each logged request, compute `max_cost = output_tokens * max_output_rate_for_model / 1000`. Savings = `SUM(max_cost - actual_cost)`. Requires joining against provider config (rates). Valuable for justifying arbstr's existence to users. |

### Differentiator Priority

1. **Time-series bucketed data** -- Medium complexity but high value. Without this, users cannot see trends. Essential for any dashboard or monitoring integration.
2. **Request log listing with pagination** -- Low complexity, high utility for debugging. A natural complement to aggregate stats.
3. **Group-by parameter** -- Avoids endpoint proliferation. One flexible endpoint instead of separate per-model, per-provider endpoints.
4. **Latency percentiles** -- Medium complexity due to SQLite limitations, but percentiles are standard in monitoring. Defer if performance is a concern with large datasets.
5. **Top-N expensive requests** -- Nearly free once log listing exists.
6. **Cost savings estimate** -- Compelling "arbstr paid for itself" metric but requires more complex queries.

---

## Anti-Features

Features to deliberately NOT build for the analytics API.

| Anti-Feature | Why Other Products Have It | Why arbstr Should NOT Build It | What to Do Instead |
|--------------|---------------------------|-------------------------------|--------------------|
| **GraphQL analytics API** | Kong uses GraphQL for flexible aggregate queries. Allows clients to request exactly the fields and groupings they need without multiple REST endpoints. | arbstr is a single-user local proxy, not a multi-tenant SaaS platform. GraphQL adds a dependency (async-graphql or juniper), schema maintenance, and complexity disproportionate to the use case. The query patterns are predictable and finite. | Use REST endpoints with query parameters for filtering and grouping. The number of useful query patterns is small enough to serve with 2-3 endpoints. |
| **Real-time streaming analytics (WebSocket/SSE)** | Datadog and Helicone offer real-time dashboards with live-updating metrics. | arbstr runs locally. The user is making requests and can refresh manually. Real-time push adds complexity (connection management, state synchronization) for marginal value in a local tool. | Serve point-in-time HTTP responses. Clients poll if they want updates. |
| **User/team/organization multi-tenancy in analytics** | LiteLLM has hierarchical budget tracking (Org > Team > User > Key). Portkey tracks per-user spend. | arbstr is a single-user local proxy. There is no concept of teams, organizations, or API keys. The `requests` table has no user column (other than the OpenAI `user` field, which is optional and client-controlled). | If user-level tracking is needed later, add it as a filter on the existing `user` field from OpenAI requests. Do not build a multi-tenant auth system. |
| **Export to CSV/Prometheus/OpenTelemetry** | Enterprise API gateways export metrics to monitoring stacks. | Premature integration. arbstr's SQLite database is directly accessible for any export tool. Adding export formats increases maintenance surface for each output format. | Expose JSON endpoints. Users who need Prometheus metrics can use a sidecar exporter that queries the JSON API. The SQLite file is also directly queryable. |
| **Budget alerts and notifications** | LiteLLM and Portkey support budget limits with email/webhook alerts when spend exceeds thresholds. | arbstr is a local proxy without a notification infrastructure. Budget enforcement (blocking requests over a limit) is a separate feature from querying past costs. Mixing enforcement into query endpoints conflates read and write concerns. | Build query endpoints as pure reads. Budget enforcement, if desired, belongs in the policy engine as a separate milestone. |
| **Caching/materialized views for analytics** | Large-scale analytics platforms pre-compute aggregates for performance. | arbstr's SQLite database is local and small. A single-user proxy might generate hundreds to low thousands of requests per day. SQLite can aggregate tens of thousands of rows in milliseconds. Pre-computation adds complexity (cache invalidation, staleness) without measurable benefit. | Use direct SQL queries against the `requests` table. Add indexes if specific queries prove slow. SQLite handles this scale trivially. |
| **Custom date format support** | Some APIs accept multiple date formats (unix epoch, ISO 8601, relative strings like "2 hours ago"). | Supporting multiple formats increases parsing complexity and testing surface. Pick one format and document it. | Use ISO 8601 exclusively (the format arbstr already stores). Accept `range` presets as the only shorthand. |

---

## Feature Dependencies

```
GET /stats (aggregate summary)
  |
  +-- Time range filtering (?start=...&end=... or ?range=last_24h)
  |     |
  |     +-- Time-series buckets (?bucket=1h) [differentiator]
  |
  +-- Per-model breakdown (?group_by=model)
  |
  +-- Per-provider breakdown (?group_by=provider)
  |
  +-- Success rate (included in aggregate fields)

GET /stats/requests (log listing) [differentiator]
  |
  +-- Pagination (?page=1&per_page=50)
  |
  +-- Filtering (?model=gpt-4o&provider=alpha&success=true)
  |
  +-- Top-N expensive (?sort=cost_desc&per_page=10)

Latency percentiles (p50/p95/p99) [differentiator]
  |
  +-- Included in aggregate response when feasible
  |
  +-- OR separate field in summary (requires in-app computation)

Database indexes
  |
  +-- idx_requests_timestamp (already exists)
  |
  +-- idx_requests_model (NEW -- speeds up GROUP BY model)
  |
  +-- idx_requests_provider (NEW -- speeds up GROUP BY provider)
```

Key dependency insight: **The aggregate summary endpoint is the foundation.** Time range filtering and group-by are parameters on that endpoint, not separate endpoints. Build the aggregate endpoint first with hardcoded groupings (model, provider), then generalize to a `group_by` parameter if needed.

---

## Endpoint Design Recommendations

Based on patterns from OpenAI Usage API, LiteLLM, and Helicone, here are the recommended endpoints and response shapes.

### Endpoint 1: `GET /stats` -- Aggregate Summary

The core endpoint. Returns aggregate metrics over a time range with optional grouping.

**Query Parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `start` | ISO 8601 string | (none = all time) | Inclusive start of time range |
| `end` | ISO 8601 string | (none = now) | Exclusive end of time range |
| `range` | enum string | (none) | Preset: `last_1h`, `last_24h`, `last_7d`, `last_30d`. Ignored if `start`/`end` provided. |
| `group_by` | comma-separated | (none) | Fields to group by: `model`, `provider`, `policy`, `streaming` |
| `model` | string | (none) | Filter to specific model |
| `provider` | string | (none) | Filter to specific provider |

**Response (no grouping):**

```json
{
  "period": {
    "start": "2026-02-15T00:00:00Z",
    "end": "2026-02-16T00:00:00Z"
  },
  "stats": {
    "total_requests": 1247,
    "total_cost_sats": 34521.50,
    "total_input_tokens": 2450000,
    "total_output_tokens": 890000,
    "avg_latency_ms": 1823.4,
    "avg_cost_sats": 27.68,
    "success_rate": 0.982,
    "total_errors": 22,
    "streaming_requests": 980
  }
}
```

**Response (with `?group_by=model`):**

```json
{
  "period": {
    "start": "2026-02-15T00:00:00Z",
    "end": "2026-02-16T00:00:00Z"
  },
  "stats": {
    "total_requests": 1247,
    "total_cost_sats": 34521.50
  },
  "groups": [
    {
      "model": "gpt-4o",
      "total_requests": 800,
      "total_cost_sats": 28000.00,
      "total_input_tokens": 1800000,
      "total_output_tokens": 600000,
      "avg_latency_ms": 2100.5,
      "avg_cost_sats": 35.00,
      "success_rate": 0.975
    },
    {
      "model": "gpt-4o-mini",
      "total_requests": 447,
      "total_cost_sats": 6521.50,
      "total_input_tokens": 650000,
      "total_output_tokens": 290000,
      "avg_latency_ms": 1330.2,
      "avg_cost_sats": 14.59,
      "success_rate": 0.996
    }
  ]
}
```

**Design rationale:** This follows OpenAI's pattern of a single endpoint with `group_by` rather than separate `/stats/models` and `/stats/providers` endpoints. The `period` object makes the effective time range explicit (important when presets are used). The top-level `stats` always contains the un-grouped totals even when groups are present -- this follows LiteLLM's daily activity response pattern.

### Endpoint 2: `GET /stats/timeseries` -- Bucketed Time Series (Differentiator)

Returns the same aggregate fields bucketed by time period. Essential for plotting trends.

**Query Parameters:** Same as `/stats` plus:

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `bucket` | enum string | `1d` | Bucket width: `1h`, `1d` |

**Response:**

```json
{
  "period": {
    "start": "2026-02-09T00:00:00Z",
    "end": "2026-02-16T00:00:00Z"
  },
  "bucket": "1d",
  "data": [
    {
      "start": "2026-02-09T00:00:00Z",
      "end": "2026-02-10T00:00:00Z",
      "total_requests": 156,
      "total_cost_sats": 4200.00,
      "avg_latency_ms": 1900.0,
      "success_rate": 0.99
    },
    {
      "start": "2026-02-10T00:00:00Z",
      "end": "2026-02-11T00:00:00Z",
      "total_requests": 203,
      "total_cost_sats": 5100.00,
      "avg_latency_ms": 1750.0,
      "success_rate": 0.985
    }
  ]
}
```

### Endpoint 3: `GET /stats/requests` -- Request Log Listing (Differentiator)

Browse individual requests with filtering and pagination.

**Query Parameters:**

| Param | Type | Default | Description |
|-------|------|---------|-------------|
| `start` / `end` / `range` | as above | as above | Time range |
| `model` | string | (none) | Filter by model |
| `provider` | string | (none) | Filter by provider |
| `success` | bool | (none) | Filter by success |
| `streaming` | bool | (none) | Filter by streaming |
| `sort` | enum | `timestamp_desc` | Sort: `timestamp_desc`, `timestamp_asc`, `cost_desc`, `latency_desc` |
| `page` | integer | 1 | Page number |
| `per_page` | integer | 50 | Results per page (max 100) |

**Response:**

```json
{
  "period": {
    "start": "2026-02-15T00:00:00Z",
    "end": "2026-02-16T00:00:00Z"
  },
  "pagination": {
    "page": 1,
    "per_page": 50,
    "total": 1247,
    "total_pages": 25
  },
  "data": [
    {
      "correlation_id": "550e8400-e29b-41d4-a716-446655440000",
      "timestamp": "2026-02-15T14:32:01Z",
      "model": "gpt-4o",
      "provider": "provider-alpha",
      "policy": null,
      "streaming": false,
      "input_tokens": 150,
      "output_tokens": 300,
      "cost_sats": 42.50,
      "latency_ms": 1523,
      "stream_duration_ms": null,
      "success": true,
      "error_message": null
    }
  ]
}
```

---

## Time Range Conventions

Based on research across OpenAI, LiteLLM, Prometheus, GitBook, and Adobe Analytics APIs:

| Approach | Used By | Convention | Recommended for arbstr |
|----------|---------|------------|----------------------|
| Unix timestamps (seconds) | OpenAI Usage API, Prometheus | `start_time=1730419200` | No -- harder for humans to construct in curl/browser |
| ISO 8601 strings | LiteLLM, general REST APIs | `start_date=2026-02-15` | **Yes** -- matches arbstr's existing timestamp format, human-readable |
| Preset enums | GitBook, Adobe, Helicone dashboards | `range=last_24h` | **Yes** -- convenience shorthand |
| Relative durations | Prometheus, Grafana | `duration=24h` | No -- less common in REST APIs, harder to validate |

**Recommendation:** Accept ISO 8601 strings for `start`/`end` and preset enums for `range`. This balances human usability (easy to construct in curl), machine usability (parseable), and compatibility with existing timestamp format in the database.

**Presets to support:** `last_1h`, `last_24h`, `last_7d`, `last_30d`, `all`. These cover the most common monitoring windows and map to established patterns from Helicone and GitBook's analytics APIs.

---

## MVP Recommendation

For the first version of cost querying endpoints:

### Must Have

1. **`GET /stats` with aggregate summary** -- The core endpoint. Returns total_requests, total_cost_sats, total_input_tokens, total_output_tokens, avg_latency_ms, success_rate, total_errors, streaming_requests. No grouping in v1. Single SQL query, simple handler.

2. **Time range filtering on `/stats`** -- `?start=...&end=...` and `?range=last_24h`. Resolves presets server-side. Adds `WHERE` clause to query.

3. **Per-model breakdown on `/stats`** -- `?group_by=model`. Returns the same aggregate fields grouped by model name. Essential for answering "which model costs the most?"

4. **Per-provider breakdown on `/stats`** -- `?group_by=provider`. Returns aggregate fields grouped by provider. Essential for arbstr's multi-provider value proposition.

### Should Have

5. **`GET /stats/requests` with pagination** -- Browse individual request logs. Low complexity, high debugging value.

6. **Preset time ranges** -- `?range=last_1h|last_24h|last_7d|last_30d`. Convenience feature that avoids requiring users to compute ISO timestamps.

7. **Model/provider filter params** -- `?model=gpt-4o` and `?provider=alpha` on the `/stats` endpoint. Narrow aggregates to a specific model or provider without needing group_by.

### Defer

8. **Time-series bucketed data** (`GET /stats/timeseries`) -- Medium complexity (SQLite `strftime` grouping), only valuable once someone builds a dashboard or monitoring integration. Ship it in a follow-up.

9. **Latency percentiles (p50/p95/p99)** -- Requires either loading all latency values into memory or a SQLite extension. Defer until aggregate latency proves insufficient.

10. **Cost savings estimate** -- Requires cross-referencing request data with provider config rates. Compelling but not essential for basic cost visibility.

---

## Complexity Estimates

| Feature | New Code (est.) | New Files | Touches Existing | Risk |
|---------|----------------|-----------|-----------------|------|
| Aggregate summary handler | ~80 lines | stats handler module | server.rs (add route) | Low -- single SQL query, JSON response |
| Time range filtering | ~40 lines | (in stats module) | (none) | Low -- WHERE clause + date parsing |
| Preset time ranges | ~30 lines | (in stats module) | (none) | Low -- enum match to UTC calculations |
| Group-by model/provider | ~60 lines | (in stats module) | (none) | Low -- GROUP BY in SQL, array response |
| Request log listing | ~80 lines | (in stats module) | server.rs (add route) | Low -- paginated SELECT |
| New DB indexes | ~5 lines | new migration | (none) | Low -- CREATE INDEX IF NOT EXISTS |
| Query parameter types | ~50 lines | (in stats module) | (none) | Low -- serde deserialize structs for axum |

**Total estimated new code:** ~280 lines for Must Have, ~400 lines with Should Have.

**New migration required:** Yes, for `CREATE INDEX` on `model` and `provider` columns to speed up GROUP BY queries.

---

## Sources

- [OpenAI Usage API Reference](https://platform.openai.com/docs/api-reference/usage) -- HIGH confidence. Documents `/organization/usage/completions` with `bucket_width` (1m/1h/1d), `group_by` (model, project_id, user_id), `start_time`/`end_time` parameters, and bucketed response format.
- [OpenAI Costs API Reference](https://developers.openai.com/api/reference/resources/organization/subresources/audit_logs/methods/get_costs) -- HIGH confidence. Documents `/organization/costs` endpoint with `bucket_width`, `group_by` (line_item, project_id), response with `amount.value` and `amount.currency`.
- [OpenAI Usage API Cookbook](https://cookbook.openai.com/examples/completions_usage_api) -- HIGH confidence. Practical examples of querying usage and cost data programmatically.
- [LiteLLM Spend Tracking](https://docs.litellm.ai/docs/proxy/cost_tracking) -- MEDIUM confidence. Documents `/global/spend/report` with `start_date`/`end_date`, `/user/daily/activity` with per-model and per-provider breakdown response.
- [LiteLLM Daily Spend Metrics](https://docs.litellm.ai/docs/proxy/metrics) -- MEDIUM confidence. Documents daily spend and usage metrics endpoint.
- [Helicone Cost Tracking](https://docs.helicone.ai/guides/cookbooks/cost-tracking) -- MEDIUM confidence. Documents cost analytics patterns: per-model, per-request, per-user cost tracking.
- [Portkey Cost Management](https://portkey.ai/docs/product/observability/cost-management) -- MEDIUM confidence. Documents analytics tab with filtering by provider, model, and timeframe.
- [Moesif REST API Design - Filtering, Sorting, Pagination](https://www.moesif.com/blog/technical/api-design/REST-API-Design-Filtering-Sorting-and-Pagination/) -- MEDIUM confidence. General REST API design patterns for query parameters.
- [GitBook Events Aggregation API](https://gitbook.com/docs/guides/docs-analytics/track-advanced-analytics-with-gitbooks-events-aggregation-api) -- MEDIUM confidence. Documents preset time ranges: lastYear, last3Months, last30Days, last7Days, last24Hours.
- [Prometheus Query Range API](https://prometheus.io/docs/prometheus/latest/querying/basics/) -- HIGH confidence. Documents `start`, `end`, `step` parameters for time-series bucketing.
- [P50 vs P95 vs P99 Latency](https://oneuptime.com/blog/post/2025-09-15-p50-vs-p95-vs-p99-latency-percentiles/view) -- MEDIUM confidence. Explains why percentiles matter more than averages for API monitoring.
- [TrueFoundry - Observability in AI Gateway](https://www.truefoundry.com/blog/observability-in-ai-gateway) -- MEDIUM confidence. Documents common metrics tracked by AI gateways: p50/p95/p99 latency, error rates, token usage, cost.
