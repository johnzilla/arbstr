# Phase 1: Foundation - Context

**Gathered:** 2026-02-02
**Status:** Ready for planning

<domain>
## Phase Boundary

Fix the broken cost calculation to use the full formula (input_rate + output_rate + base_fee) and add unique correlation IDs to every proxied request. This is infrastructure that all subsequent phases depend on — correct costs for logging, IDs for tracing.

</domain>

<decisions>
## Implementation Decisions

### Cost estimation for routing
- Routing decision uses `output_rate + base_fee` for ranking providers (output_rate is dominant variable cost, base_fee matters for short requests)
- Input_rate is NOT used for routing because output token count is unknown at routing time
- Full formula `(input_tokens * input_rate + output_tokens * output_rate) / 1000 + base_fee` is used ONLY for actual cost logging after the response is received and token counts are known

### Correlation ID propagation
- arbstr always generates its own UUID for every request
- ID is internal to arbstr — NOT forwarded to upstream Routstr provider
- Client-supplied request IDs (X-Request-ID) are ignored — arbstr controls the ID
- ID appears in: structured tracing logs, response headers (Phase 3), SQLite log (Phase 2)

### Cost rounding behavior
- Store costs as float (REAL column in SQLite), not integer
- Display costs as float — fractional sats matter for cost optimization
- No rounding — sub-sat precision preserved throughout the system
- This means the cost_sats field in the database schema should be REAL, not INTEGER

### Claude's Discretion
- UUID format (v4 standard is fine)
- Exact tracing span structure for correlation IDs
- How to count input tokens from the request (character count estimate vs. exact)
- Test strategy for cost formula correctness

</decisions>

<specifics>
## Specific Ideas

- Routing ranking and logging cost are deliberately different calculations — routing is a heuristic (output_rate + base_fee), logging is exact (full formula with actual tokens)
- The schema in CLAUDE.md defines cost_sats as INTEGER — this needs to change to REAL to support fractional sats

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 01-foundation*
*Context gathered: 2026-02-02*
