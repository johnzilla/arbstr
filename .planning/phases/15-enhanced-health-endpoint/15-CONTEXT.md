# Phase 15: Enhanced Health Endpoint - Context

**Gathered:** 2026-02-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Enhance the existing `/health` endpoint to report per-provider circuit breaker state and overall system health status. Operators can see which providers are healthy, degraded, or down at a glance. No new endpoints -- the existing `/health` is replaced with the richer response.

</domain>

<decisions>
## Implementation Decisions

### Response structure
- Providers keyed by name as an object: `{"providers": {"alpha": {"state": "closed", "failure_count": 0}}}`
- Each provider entry includes only `state` and `failure_count` -- minimal, matches success criteria
- Top-level has only `status` field plus `providers` object -- no aggregate counts or uptime
- Circuit state values are lowercase strings: `"closed"`, `"open"`, `"half_open"`

### Status semantics
- HTTP 200 for `ok` and `degraded`, HTTP 503 only for `unhealthy` (all circuits open)
- Half-open providers count as degraded for top-level status calculation
- Zero configured providers returns `"ok"` with empty providers object -- server is running fine
- No timestamps in the response -- state and failure_count are sufficient

### Backward compatibility
- Replace existing `/health` response in-place -- clean break, new response shape entirely
- No versioning or separate endpoint -- this is a local proxy with no external consumers
- Open endpoint, no auth -- consistent with current behavior
- Content-Type: application/json, consistent with all other endpoints

### Claude's Discretion
- Internal implementation approach (how to query circuit breaker registry)
- Response serialization pattern (serde structs vs manual JSON)
- Test structure and coverage approach

</decisions>

<specifics>
## Specific Ideas

No specific requirements -- open to standard approaches

</specifics>

<deferred>
## Deferred Ideas

None -- discussion stayed within phase scope

</deferred>

---

*Phase: 15-enhanced-health-endpoint*
*Context gathered: 2026-02-16*
