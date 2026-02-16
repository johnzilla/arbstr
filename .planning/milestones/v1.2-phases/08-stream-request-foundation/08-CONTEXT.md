# Phase 8: Stream Request Foundation - Context

**Gathered:** 2026-02-15
**Status:** Ready for planning

<domain>
## Phase Boundary

Inject `stream_options: {"include_usage": true}` into upstream streaming requests so providers send usage data, and add a database UPDATE function that can write token counts and cost to an existing request log entry. No stream interception or parsing in this phase — that's Phase 9.

</domain>

<decisions>
## Implementation Decisions

### Request Mutation
- Merge with client values: if client already sends `stream_options`, preserve their settings and only add `include_usage: true` if missing (don't override)
- Always inject for streaming requests unconditionally — not gated on logging or mock mode
- Only inject when `stream: true` — non-streaming requests left completely untouched
- Injection happens at send time (when building reqwest body), not earlier in the handler

### DB Update Strategy
- Await INSERT completion before starting stream to prevent race condition (UPDATE must find the row)
- UPDATE writes `input_tokens`, `output_tokens`, `cost_sats` only — latency stays as TTFB from INSERT (Phase 10 handles full-stream latency)
- Fire-and-forget pattern (tokio::spawn, warn on failure) — consistent with existing INSERT
- Warn via tracing if UPDATE affects zero rows (rows_affected == 0) — indicates something went wrong

### Provider Compatibility
- Universal injection — no per-provider config toggle. If a provider rejects stream_options, existing retry/fallback handles the error
- If no usage data extracted from stream, still run UPDATE with NULLs to mark stream completed
- Debug-level log when provider didn't return usage data — only visible with RUST_LOG=debug
- Real provider testing deferred to Phase 10 integration — Phase 8 uses mock providers only

### Testing Approach
- Both unit tests (injection function) AND integration tests (HTTP call to mock server)
- Integration test: mock provider captures and inspects request body, asserts stream_options present in serialized JSON
- DB UPDATE tested with in-memory SQLite: INSERT row, run UPDATE, verify columns changed
- Full test suite (cargo test) must pass with zero failures before phase is done

### Claude's Discretion
- Exact placement of injection function (standalone fn vs method on request type)
- StreamOptions struct design (fields, serde attributes)
- UPDATE query construction and column selection
- Test organization (new test file vs extend existing)

</decisions>

<specifics>
## Specific Ideas

- UPDATE with NULLs on no-usage serves a dual purpose: marks stream as "completed but no usage data" vs "stream never completed" (row has no UPDATE at all)
- Merging stream_options rather than overriding respects clients that may set their own streaming preferences

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 08-stream-request-foundation*
*Context gathered: 2026-02-15*
