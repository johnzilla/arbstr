# Phase 9: SSE Stream Interception - Context

**Gathered:** 2026-02-16
**Status:** Ready for planning

<domain>
## Phase Boundary

A standalone stream wrapper module that buffers SSE lines across TCP chunk boundaries and extracts usage data from the final chunk. Observation-only — zero content mutation. The module produces a structured result that Phase 10 consumes for database updates and trailing cost events.

</domain>

<decisions>
## Implementation Decisions

### Extraction scope
- Extract usage object (prompt_tokens, completion_tokens) AND finish_reason from the final chunk
- Do NOT extract model or other metadata — just usage + finish_reason
- Return data only after the stream completes, not during streaming
- Return a structured result type (e.g., StreamResult { usage, finish_reason, done_received })
- Track whether `[DONE]` was received as part of the result — Phase 10 uses this to know the stream completed normally

### Provider variation
- Strict OpenAI SSE format only — no fallback parsing for non-standard usage locations
- If a provider deviates from OpenAI format, treat as "no usage data" — safe degradation
- Skip non-`data:` SSE lines (event:, id:, retry:) but log them at trace level for debugging
- No fallback parsing — if usage isn't where OpenAI puts it, report no usage
- Handle both `\n` and `\r\n` line endings in the SSE buffer

### Failure behavior
- If JSON parse fails on a `data:` line mid-stream, skip the bad line and continue trying — usage may still arrive in the final chunk
- Log extraction issues (malformed JSON, missing fields, unexpected format) at warn level
- If stream ends without `[DONE]` (provider disconnect, timeout), return empty result — without `[DONE]`, data is unreliable, avoid bad accounting
- Isolate extraction from the stream passthrough — catch panics so a bug in extraction never breaks the client stream

### Claude's Discretion
- Internal buffer implementation (Vec<u8>, BufRead, custom ring buffer)
- Exact structured result type naming and field types
- How to implement panic isolation in Rust (catch_unwind, separate task, etc.)
- Test fixture design for chunk-boundary scenarios

</decisions>

<specifics>
## Specific Ideas

- The "return nothing on no-DONE" decision is deliberate — the done_received flag is the trust signal. Phase 10 should not write partial/unreliable token data to the database.
- Warn-level logging for parse failures is meant to surface misbehaving providers to operators without being silenced in debug-only logs.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 09-sse-stream-interception*
*Context gathered: 2026-02-16*
