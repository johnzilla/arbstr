# Phase 10: Streaming Observability Integration - Context

**Gathered:** 2026-02-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire Phase 8 (stream_options injection, post-stream DB UPDATE) and Phase 9 (SSE observer/wrap_sse_stream) into request handlers so every streaming request logs accurate token counts, cost, full-duration latency, and completion status, with cost surfaced to clients via trailing SSE event.

</domain>

<decisions>
## Implementation Decisions

### Trailing SSE event format
- Plain `data:` line (no `event:` field) — same format as all other SSE chunks, distinguished by payload content
- Positioned after the upstream `[DONE]` passes through, followed by arbstr's own `data: [DONE]`
- JSON structure nested under key: `{"arbstr": {"cost_sats": 42, "latency_ms": 1200}}`
- Fields: cost_sats and latency_ms only — matches success criteria, minimal payload

### Completion status
- Reuse existing `success` BOOLEAN + `error_message` TEXT columns — no schema change for status
- Client disconnection detected via stream send error (broken pipe / connection reset)
- On client disconnect, continue consuming upstream to extract usage data for DB update
- Provider errors during streaming treated same as pre-stream errors — no differentiation needed

### Degradation behavior
- Always emit trailing SSE event, even when provider sends no usage data — include latency, null cost
- When usage present but cost can't be calculated (no rate configured), use `null` for cost_sats (not zero)
- Always update DB with token/cost data if extracted, regardless of client connection status
- On SseObserver panic (caught by catch_unwind), still emit trailing event with available data (latency always available, null cost)

### Latency boundaries
- Timer starts at request send time (when arbstr sends to upstream provider) — full round-trip including network
- Timer ends at last upstream byte received (before arbstr's trailing event) — measures pure provider time
- Keep both TTFB and full duration: existing `latency_ms` stays as TTFB from INSERT, add `stream_duration_ms` for full stream duration via UPDATE
- On client disconnect, latency still measures full upstream duration (not capped at disconnect time)

### Claude's Discretion
- How to wire wrap_sse_stream into the existing handler streaming path
- Error message text for different failure modes
- Trailing event serialization implementation details
- New migration for stream_duration_ms column

</decisions>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 10-streaming-observability-integration*
*Context gathered: 2026-02-16*
