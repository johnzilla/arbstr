---
phase: 02-request-logging
plan: 04
subsystem: proxy-logging
tags: [request-logging, sqlite, fire-and-forget, usage-extraction, cost-tracking]
dependency_graph:
  requires: [02-01, 02-02, 02-03]
  provides: [request-logging-integration, usage-extraction, cost-calculation-in-handler]
  affects: [03-01]
tech_stack:
  added: []
  patterns: [result-based-outcome-logging, fire-and-forget-spawn, inner-function-extraction]
key_files:
  created: []
  modified: [src/proxy/handlers.rs]
decisions:
  - id: "02-04-streaming-none"
    description: "Streaming requests logged with None tokens/cost (stream not consumed at log time)"
    rationale: "Cannot extract usage from stream body before returning response; matches CONTEXT.md decision"
  - id: "02-04-outcome-result-pattern"
    description: "Core logic extracted into execute_request returning Result<RequestOutcome, RequestError>"
    rationale: "Enables logging both success and failure paths before returning HTTP response to client"
metrics:
  duration: "2 min"
  completed: "2026-02-04"
---

# Phase 2 Plan 4: Request Logging Integration Summary

**One-liner:** Wired spawn_log_write into chat_completions handler for all code paths using Result-based outcome extraction with fire-and-forget SQLite writes.

## What Was Done

### Task 1: Restructure chat_completions handler for comprehensive logging

Rewrote the `chat_completions` handler with a clean separation between request execution and logging:

- **New types:** `RequestOutcome` (success metadata) and `RequestError` (failure metadata) capture logging data independently from HTTP response/error types
- **New functions:**
  - `execute_request`: Encapsulates provider selection, HTTP forwarding, and response parsing; returns `Result<RequestOutcome, RequestError>`
  - `handle_non_streaming_response`: Extracts usage from JSON response, calculates cost via `actual_cost_sats`, builds HTTP response
  - `handle_streaming_response`: Passes SSE chunks through with debug-level usage detection; logs with None tokens
  - `extract_usage`: Pure function extracting `(prompt_tokens, completion_tokens)` from provider response JSON
- **Logging flow:** After `execute_request` returns, the handler builds a `RequestLog` from either outcome and calls `spawn_log_write` (fire-and-forget). Response is returned after logging is initiated, not after write completes.
- **DB None guard:** If `state.db` is `None`, logging is silently skipped
- **Correlation ID:** Extracted from `Extension<RequestId>` injected by middleware (02-03)

### Task 2: Add unit tests for extract_usage

Added 4 tests covering all edge cases:
- Present usage with both fields: returns `Some((100, 200))`
- Missing usage object: returns `None`
- Partial usage (only prompt_tokens): returns `None`
- Null usage value: returns `None`

## Code Paths Logged

| Path | provider field | success | tokens | cost |
|------|---------------|---------|--------|------|
| Successful non-streaming | provider name | true | extracted from usage | calculated via actual_cost_sats |
| Successful streaming | provider name | true | None | None |
| Provider HTTP error (non-2xx) | provider name | false | None | None |
| Provider unreachable (network) | provider name | false | None | None |
| Pre-route rejection (NoProviders, NoPolicyMatch, BadRequest) | None | false | None | None |
| Response parse failure | provider name | false | None | None |

## Decisions Made

1. **Streaming logged with None tokens** (02-04-streaming-none): The stream body has not been consumed when the response is returned to the client, so usage data is unavailable. The streaming handler includes debug-level logging of usage from SSE chunks for future reference, but the formal request log uses None. This aligns with the CONTEXT.md decision.

2. **Result-based outcome pattern** (02-04-outcome-result-pattern): Rather than using try-catch or early returns, the handler delegates to `execute_request` which returns a custom Result type. Both Ok and Err variants carry logging metadata, enabling a single logging block after the match.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed clippy redundant_closure warning**
- **Found during:** Task 1
- **Issue:** `chunk.map_err(|e| std::io::Error::other(e))` flagged as redundant closure by clippy
- **Fix:** Changed to `chunk.map_err(std::io::Error::other)` (function pointer form)
- **Files modified:** src/proxy/handlers.rs

## Verification

- `cargo build`: compiles cleanly
- `cargo test`: 12 tests pass (8 existing + 4 new extract_usage tests)
- `cargo clippy -- -D warnings`: no warnings
- `spawn_log_write` called in handler
- `actual_cost_sats` used for cost calculation
- `Extension<RequestId>` in handler signature
- `extract_usage` function present
- No remaining `TODO.*Log to database` comment
- `RequestError` type used throughout error paths

## Next Phase Readiness

Phase 2 is now complete. All success criteria are met:
1. After proxying a non-streaming request, a row appears in SQLite with all required fields
2. Token counts match the provider's usage object
3. Latency reflects wall-clock time (Instant::now at handler start to elapsed at log time)
4. SQLite writes never block the response (fire-and-forget via tokio::spawn)
5. Database schema applied automatically via embedded migrations on startup

Phase 3 (Response Metadata) can proceed. The handler already sets `x-arbstr-provider` header; Phase 3 will add `x-arbstr-cost-sats`, `x-arbstr-latency-ms`, and `x-arbstr-request-id` headers.
