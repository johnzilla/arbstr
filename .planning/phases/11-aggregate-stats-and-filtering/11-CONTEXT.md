# Phase 11: Aggregate Stats and Filtering - Context

**Gathered:** 2026-02-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Read-only API endpoints that return aggregate cost and performance data from arbstr's SQLite request logs. Supports time range scoping (presets and explicit ISO 8601), model/provider filtering, and per-model grouped breakdown. Individual request log browsing is Phase 12.

</domain>

<decisions>
## Implementation Decisions

### Endpoint paths
- Stats endpoints live under `/v1/stats/*` alongside existing `/v1/chat/completions` and `/v1/models`
- Single endpoint at `/v1/stats` with `group_by=model` query param for per-model breakdown (not separate endpoints)
- Phase 12 request logs will live at `/v1/requests` (separate top-level path, not under /stats)
- Optional API key support -- not required (local proxy), but support an optional auth header

### Response shape
- Nested sections in JSON response: `counts`, `costs`, `performance` groupings (not flat)
- Minimal metadata: include `since` and `until` timestamps in response (not full filter echo)
- Per-model grouped results use object keyed by model name: `{"models": {"gpt-4o": {"counts": {...}, "costs": {...}}, ...}}`
- Include all known/configured models in grouped results, even those with zero traffic in the queried window

### Default behavior
- Default time range when no params specified: last 7 days (`last_7d`)
- Empty results return zeroed stats with `"empty": true` and a `"message"` field alongside the data
- If both `range` preset and explicit `since`/`until` provided, explicit params win (override preset)
- Presets computed from server clock in UTC at request time

### Filter semantics
- Model and provider filters use exact match only (no prefix/partial matching)
- Matching is case-insensitive (model=GPT-4O matches stored gpt-4o)
- Single filter value only per parameter (no comma-separated or repeated params)
- Filtering by a non-existent model or provider returns 404 (helps catch typos)

### Claude's Discretion
- SQL query structure and optimization
- Exact nested field names within counts/costs/performance sections
- Error response format (should follow existing OpenAI-compatible pattern)
- Read-only connection pool implementation details

</decisions>

<specifics>
## Specific Ideas

No specific requirements -- open to standard approaches within the decisions above.

</specifics>

<deferred>
## Deferred Ideas

None -- discussion stayed within phase scope.

</deferred>

---

*Phase: 11-aggregate-stats-and-filtering*
*Context gathered: 2026-02-16*
