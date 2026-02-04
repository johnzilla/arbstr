---
phase: 04-retry-and-fallback
plan: 03
subsystem: api
tags: [retry, fallback, timeout, idempotency, handlers, integration]

# Dependency graph
requires:
  - phase: 04-retry-and-fallback
    provides: select_candidates returning ordered Vec<SelectedProvider> for fallback
  - phase: 04-retry-and-fallback
    provides: retry_with_fallback() with Arc<Mutex> attempt tracking and HasStatusCode trait
provides:
  - Retry-integrated chat_completions handler with send_to_provider extraction
  - Non-streaming requests retried with backoff and fallback to alternate provider
  - x-arbstr-retries header on responses that involved retries
  - Idempotency-Key header sent to upstream providers
  - 30-second timeout returning 504 with attempt history
affects: [future circuit breaker, future rate limit retry]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Streaming/non-streaming path split in handler for different retry behaviors"
    - "timeout_at wrapping retry_with_fallback with shared Arc<Mutex> attempt tracking surviving cancellation"
    - "Idempotency-Key header for upstream request deduplication"
    - "504 Gateway Timeout with status override on Error::Provider"

key-files:
  created: []
  modified:
    - src/proxy/handlers.rs

key-decisions:
  - "Error::Provider maps to 502 by default; timeout path overrides status to 504 via status_mut()"
  - "Routing errors (no providers/no policy match) handled before retry loop, not inside it"
  - "Streaming path continues to use execute_request (no retry); non-streaming uses retry_with_fallback + send_to_provider directly"

patterns-established:
  - "send_to_provider: reusable async function for single provider request with Idempotency-Key"
  - "Path split pattern: streaming vs non-streaming determined early, separate code paths"
  - "Timeout response: create Error, into_response(), then override status_mut() for 504"

# Metrics
duration: 3min
completed: 2026-02-04
---

# Phase 4 Plan 3: Handler Integration Summary

**Retry-integrated chat_completions handler with send_to_provider extraction, 30s timeout, Idempotency-Key, and x-arbstr-retries header on retried responses**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-04T13:03:36Z
- **Completed:** 2026-02-04T13:06:53Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Extracted `send_to_provider` as reusable async function with `Idempotency-Key` header for upstream request deduplication
- Split `chat_completions` handler into streaming (no retry, fail fast) and non-streaming (retry+fallback with 30s deadline) paths
- Wired `retry_with_fallback` from retry module into non-streaming path with `select_candidates` for ordered provider list
- Shared `Arc<Mutex<Vec<AttemptRecord>>>` survives timeout cancellation, enabling `x-arbstr-retries` header even on 504 responses
- Made `RequestOutcome` and `RequestError` `pub(crate)` with `HasStatusCode` impl for retry module integration

## Task Commits

Each task was committed atomically:

1. **Task 1: Extract send_to_provider and implement HasStatusCode** - `c474c57` (refactor)
2. **Task 2: Wire retry_with_fallback into chat_completions for non-streaming** - `ecfa8b4` (feat)

## Files Created/Modified
- `src/proxy/handlers.rs` - Extracted `send_to_provider`, split handler into streaming/non-streaming paths, wired retry+fallback with timeout, added `ARBSTR_RETRIES_HEADER` and `RETRY_TIMEOUT` constants

## Decisions Made
- `Error::Provider` maps to 502 Bad Gateway by default in `IntoResponse`; for timeout, the response status is overridden to 504 via `status_mut()` rather than adding a new error variant
- Routing errors (no providers, no policy match) are handled before entering the retry loop, not inside it -- these are permanent errors that should not be retried
- Streaming path continues to use `execute_request` (single provider selection, no retry) while non-streaming path bypasses it entirely, using `select_candidates` + `retry_with_fallback` + `send_to_provider` directly

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 4 (Retry and Fallback) is now complete -- all 3 plans delivered
- Full retry+fallback chain operational: candidate selection (04-01), retry module (04-02), handler integration (04-03)
- All 33 project tests pass
- Manual testing with mock server confirms retry behavior with backoff, fallback to alternate provider, and x-arbstr-retries header

---
*Phase: 04-retry-and-fallback*
*Completed: 2026-02-04*
