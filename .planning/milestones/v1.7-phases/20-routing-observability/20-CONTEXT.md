# Phase 20: Routing Observability - Context

**Gathered:** 2026-04-09
**Status:** Ready for planning

<domain>
## Phase Boundary

Make complexity score and tier visible across all observability surfaces: response headers, trailing SSE metadata, DB columns, stats group_by=tier, and INFO-level logging. All additive work — no routing or scoring logic changes.

</domain>

<decisions>
## Implementation Decisions

### Response headers
- **D-01:** Add `x-arbstr-complexity-score` header to non-streaming responses. Value: 3 decimal places (e.g., `0.423`). Omit header when score is None (header override path).
- **D-02:** Add `x-arbstr-tier` header to non-streaming responses. Value: lowercase tier string (`local`, `standard`, `frontier`).
- **D-03:** Both headers added alongside existing `x-arbstr-cost-sats`, `x-arbstr-latency-ms` headers in the response builder.

### Trailing SSE metadata
- **D-04:** Extend the trailing SSE metadata event (already exists from v1.2) with `complexity_score` and `tier` fields.
- **D-05:** Score formatted to 3 decimal places in SSE metadata. Tier as lowercase string.

### Database schema
- **D-06:** New migration adding `complexity_score REAL` (nullable) and `tier TEXT` (nullable) columns to `requests` table.
- **D-07:** Columns are nullable — old rows predate scoring and have NULL. New rows always populated.
- **D-08:** INSERT and UPDATE statements in storage/logging.rs extended to include both new columns.

### Stats endpoint
- **D-09:** `GET /v1/stats?group_by=tier` returns per-tier cost/performance breakdown, same shape as existing `group_by=model` and `group_by=provider`.
- **D-10:** Tier breakdown query uses `tier` column with COALESCE for NULL handling (group as "unknown" or exclude NULLs).

### Logging
- **D-11:** Each request logs complexity score, matched tier, and selected provider at INFO level via tracing.
- **D-12:** Log format: `tracing::info!(complexity_score = %score, tier = %tier, provider = %provider, "Request routed")`

### Claude's Discretion
- Whether to add score/tier to streaming response headers (values not known at header-send time, same as cost)
- Exact SQL for tier breakdown query
- Whether to add indexes on tier column
- Whether to extend /v1/requests log listing with tier/score filter params

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Response headers (pattern to follow)
- `src/proxy/handlers.rs` -- existing `x-arbstr-cost-sats`, `x-arbstr-latency-ms` header injection
- `src/proxy/handlers.rs` -- `ResolvedCandidates` struct with `complexity_score` and `tier` fields (from Phase 19)

### SSE trailing metadata
- `src/proxy/stream.rs` -- `wrap_sse_stream` and trailing SSE event with arbstr metadata

### Database
- `migrations/` -- existing migration files for schema pattern
- `src/storage/logging.rs` -- INSERT/UPDATE statements for request log
- `src/storage/stats.rs` -- aggregate query patterns, group_by implementation
- `src/storage/logs.rs` -- paginated log queries

### Stats endpoint
- `src/proxy/stats.rs` -- `StatsQuery`, `StatsResponse`, group_by handling

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `ResolvedCandidates.complexity_score: Option<f64>` and `ResolvedCandidates.tier: Tier` already populated by Phase 19
- Header injection pattern: `response.headers_mut().insert(HeaderName, HeaderValue)` used extensively
- Trailing SSE event format: `data: {"arbstr": {"cost_sats": ..., "latency_ms": ...}}`
- Stats group_by: `match group_by { "model" => ..., "provider" => ... }` pattern in stats.rs

### Established Patterns
- Migrations use sequential numbering in `migrations/` directory
- DB writer uses bounded mpsc channel — INSERT happens async
- Stats queries use column name whitelist for SQL injection prevention
- Log queries support dynamic WHERE clauses with parameter binding

### Integration Points
- `chat_completions` handler has access to `ResolvedCandidates` with score/tier
- Non-streaming path builds response headers after provider response
- Streaming path sends trailing event after upstream [DONE]
- DB writer receives log entries via channel

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 20-routing-observability*
*Context gathered: 2026-04-09*
