---
phase: 03-response-metadata
plan: 01
subsystem: proxy-headers
tags: [response-headers, metadata, cost-tracking, latency, correlation-id, streaming]
dependency_graph:
  requires: [02-04]
  provides: [response-metadata-headers, attach-arbstr-headers-helper]
  affects: [04-01]
tech_stack:
  added: []
  patterns: [centralized-header-helper, error-as-ok-response]
key_files:
  created: []
  modified: [src/proxy/handlers.rs]
decisions:
  - id: "03-01-error-ok-pattern"
    description: "Error path returns Ok(error_response) with arbstr headers instead of Err(Error)"
    rationale: "IntoResponse trait cannot accept additional context; building response inline allows header attachment"
  - id: "03-01-streaming-omit-cost-latency"
    description: "Streaming responses omit cost and latency headers, include streaming flag"
    rationale: "Cost and latency not known at header-send time for streaming responses"
  - id: "03-01-cost-2dp"
    description: "Cost formatted with 2 decimal places (e.g. 0.10 not 0.1)"
    rationale: "Matches CONTEXT.md example format; consistent decimal representation"
metrics:
  duration: "2 min"
  completed: "2026-02-04"
---

# Phase 3 Plan 1: Response Metadata Headers Summary

**One-liner:** Centralized attach_arbstr_headers helper injects request-id, cost, latency, provider, and streaming headers on all chat completion response paths (success, streaming, error).

## What Was Done

### Task 1: Add header constants, helper function, and restructure response paths

- **5 header constants** added alongside existing ARBSTR_POLICY_HEADER: REQUEST_ID, COST_SATS, LATENCY_MS, PROVIDER, STREAMING
- **attach_arbstr_headers helper** function handles all three response scenarios:
  - Non-streaming: request-id, latency, provider, cost (if known)
  - Streaming: request-id, provider, streaming flag (omits cost/latency)
  - Error: request-id, latency, provider (if known), no cost
- **Restructured chat_completions return**: Both Ok and Err arms now produce `Ok(Response)` with arbstr headers attached. Error responses use `error.into_response()` then mutate headers via `headers_mut()`.
- **Removed duplicate x-arbstr-provider** from `handle_non_streaming_response` and `handle_streaming_response` Response::builder chains. The helper function now owns all arbstr header insertion.
- Added `HeaderName` and `HeaderValue` to the axum::http import.

### Task 2: Add unit tests for attach_arbstr_headers

5 unit tests covering all header attachment scenarios:

| Test | Scenario | Verifies |
|------|----------|----------|
| test_attach_headers_non_streaming | Full success response | All 4 headers present, no streaming flag |
| test_attach_headers_streaming | Streaming success | request-id + provider + streaming flag, no cost/latency |
| test_attach_headers_error_no_provider | Pre-route error | request-id + latency only, no provider/cost/streaming |
| test_attach_headers_no_cost | Success without usage data | request-id + latency + provider, no cost |
| test_attach_headers_cost_formatting | Decimal precision | "0.10" not "0.1" |

## Task Commits

| Task | Commit | Description |
|------|--------|-------------|
| 1 | 9942ffe | feat(03-01): add response metadata headers with centralized helper |
| 2 | 177ed06 | test(03-01): add unit tests for attach_arbstr_headers helper |

## Files Modified

- `src/proxy/handlers.rs` - Added 5 header constants, attach_arbstr_headers helper, restructured chat_completions return, removed duplicate provider headers, added 5 unit tests

## Decisions Made

1. **Error path returns Ok(error_response)** (03-01-error-ok-pattern): The `IntoResponse` trait takes only `self` with no mechanism to pass request-scoped metadata. The handler now converts errors into responses inline, attaches headers, and returns `Ok(response)`. Pre-handler errors (JSON parse failures from axum's `Json` extractor) still produce error responses without arbstr headers -- this is expected and acceptable.

2. **Streaming omits cost and latency** (03-01-streaming-omit-cost-latency): Streaming responses cannot include cost or latency headers because these values are not known when HTTP response headers are sent (stream has not been consumed). The `x-arbstr-streaming: true` header signals this to clients.

3. **Cost formatted with 2 decimal places** (03-01-cost-2dp): Uses `format!("{:.2}", cost)` for consistent representation matching the CONTEXT.md example ("42.35"). Sub-sat precision loss at 2dp is negligible for cost tracking.

## Deviations from Plan

None -- plan executed exactly as written.

## Issues Encountered

None.

## Verification

- `cargo build`: compiles cleanly
- `cargo clippy -- -D warnings`: no warnings
- `cargo test`: 17 tests pass (12 existing + 5 new)
- `x-arbstr-provider` appears only once in handlers.rs (constant declaration)
- All 6 ARBSTR_*_HEADER constants present
- No duplicate provider header in Response::builder chains

## Next Phase Readiness

Phase 3 is now complete (single plan). All success criteria met:
1. Non-streaming responses include x-arbstr-request-id, x-arbstr-cost-sats, x-arbstr-latency-ms, and x-arbstr-provider headers
2. Streaming responses include x-arbstr-request-id, x-arbstr-provider, and x-arbstr-streaming: true -- but NOT cost or latency
3. Error responses include x-arbstr-request-id and x-arbstr-latency-ms, plus x-arbstr-provider when known
4. Headers visible to standard HTTP clients (standard HTTP response headers)

Phase 4 (Retry and Fallback) can proceed. The response metadata infrastructure is in place for retry-related headers (x-arbstr-retries).
