# Phase 3: Response Metadata - Context

**Gathered:** 2026-02-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Expose per-request cost, latency, provider, and correlation ID to clients via HTTP response headers on every proxied response. Streaming and non-streaming responses have different header sets. No new endpoints, no config options — headers are always present.

</domain>

<decisions>
## Implementation Decisions

### Header naming & format
- Prefix: `X-Arbstr-*` (consistent with existing `X-Arbstr-Policy` request header)
- `X-Arbstr-Request-Id`: Full UUID v4 (e.g. `550e8400-e29b-41d4-a716-446655440000`) — matches structured logs and SQLite
- `X-Arbstr-Cost-Sats`: Decimal sats with sub-satoshi precision (e.g. `42.35`) — matches f64 `actual_cost_sats` from Phase 1
- `X-Arbstr-Latency-Ms`: Integer milliseconds (e.g. `1523`) — sub-ms precision not meaningful for proxy latency
- `X-Arbstr-Provider`: Provider name string (e.g. `provider-alpha`)
- No `X-Arbstr-Model` header — model is already in the OpenAI response body
- No `X-Arbstr-Policy` response header — policy is an internal routing detail

### Streaming behavior
- Streaming responses include `X-Arbstr-Request-Id` (known before stream starts)
- Streaming responses include `X-Arbstr-Streaming: true` so clients know which headers to expect
- Streaming responses **omit** `X-Arbstr-Cost-Sats` and `X-Arbstr-Latency-Ms` (not known until stream ends)
- SSE-based cost/latency delivery at end of stream is **out of scope** — deferred to future streaming observability work

### Exposure policy
- All headers always on — no opt-in config, no per-request toggle
- Single-user proxy, no reason to hide metadata

### Error response headers
- `X-Arbstr-Request-Id`: Always present on error responses (errors are logged with this ID)
- `X-Arbstr-Latency-Ms`: Always present on error responses (shows time before failure — timeout vs immediate rejection)
- `X-Arbstr-Cost-Sats`: Present if cost is known (tokens consumed before error), omitted otherwise
- `X-Arbstr-Provider`: Always present on error responses (shows which provider failed)

### Claude's Discretion
- Header insertion implementation pattern (middleware vs inline in handler)
- How to measure latency (where to start/stop the timer)
- Header ordering

</decisions>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches.

</specifics>

<deferred>
## Deferred Ideas

- SSE comment-based cost/latency at end of streaming responses — future streaming observability phase (aligns with existing OBSRV-12 deferral)

</deferred>

---

*Phase: 03-response-metadata*
*Context gathered: 2026-02-03*
