---
phase: 04-retry-and-fallback
plan: 02
subsystem: api
tags: [retry, fallback, backoff, tokio, async]

# Dependency graph
requires:
  - phase: 04-retry-and-fallback
    provides: select_candidates returning ordered Vec<SelectedProvider> for fallback
provides:
  - retry_with_fallback() async function with generic closure interface
  - AttemptRecord, CandidateInfo, HasStatusCode, RetryOutcome types
  - is_retryable() for 5xx status code classification
  - format_retries_header() for x-arbstr-retries header construction
affects: [04-03 handler integration, future circuit breaker]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Arc<Mutex<Vec<AttemptRecord>>> for timeout-safe attempt tracking"
    - "Generic closure parameter for testable retry logic without handler dependencies"
    - "HasStatusCode trait for decoupled error inspection"

key-files:
  created:
    - src/proxy/retry.rs
  modified:
    - src/proxy/mod.rs

key-decisions:
  - "BACKOFF_DURATIONS uses [Duration; 3] = [1s, 2s, 4s] matching locked decision, though only 1s and 2s used at runtime with MAX_RETRIES=2"
  - "retry_with_fallback is generic over T/E with HasStatusCode trait, not coupled to RequestOutcome/RequestError"
  - "Attempts tracked via Arc<Mutex<Vec<AttemptRecord>>> parameter so caller owns the vec and can read it after timeout cancellation"
  - "RetryOutcome has no attempts field -- attempts live in the shared Arc, not the return value"

patterns-established:
  - "HasStatusCode trait: retry module inspects error status without depending on handler types"
  - "CandidateInfo: lightweight struct decouples retry from router SelectedProvider"
  - "Shared Arc<Mutex<Vec>> pattern for data that must survive async cancellation"

# Metrics
duration: 2min
completed: 2026-02-04
---

# Phase 4 Plan 2: Retry Module Summary

**Generic retry-with-fallback loop using Arc<Mutex> attempt tracking, fixed 1s/2s backoff, and HasStatusCode trait for handler-independent testability**

## Performance

- **Duration:** 2 min
- **Started:** 2026-02-04T12:58:53Z
- **Completed:** 2026-02-04T13:00:53Z
- **Tasks:** 1
- **Files modified:** 2

## Accomplishments
- Created self-contained retry module with `retry_with_fallback()` accepting a generic async closure
- Implemented `is_retryable()` for 500/502/503/504 classification and `format_retries_header()` for compact header format
- Designed `Arc<Mutex<Vec<AttemptRecord>>>` pattern so attempt history survives timeout cancellation
- All 11 unit tests passing covering happy path, retry sequences, fallback, non-retryable fast-fail, and backoff timing verification

## Task Commits

Each task was committed atomically:

1. **Task 1: Create retry module with core types and functions** - `afa0585` (feat)

## Files Created/Modified
- `src/proxy/retry.rs` - Core retry/fallback logic: retry_with_fallback(), AttemptRecord, CandidateInfo, HasStatusCode, RetryOutcome, is_retryable(), format_retries_header(), 11 unit tests
- `src/proxy/mod.rs` - Added `pub mod retry;` declaration

## Decisions Made
- Used `[Duration; 3]` for BACKOFF_DURATIONS to document the full 1s/2s/4s sequence even though only first two slots are used with MAX_RETRIES=2
- Made retry_with_fallback generic (T, E, F, Fut) rather than coupled to handler types, enabling isolated unit testing with mock errors
- Attempt history tracked via shared Arc<Mutex<Vec>> parameter (not return value) to survive timeout cancellation by the caller
- Non-retryable errors (4xx) skip fallback entirely -- fail fast on permanent errors

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Added Debug derive to MockError test type**
- **Found during:** Task 1 (unit test compilation)
- **Issue:** `MockError` lacked `#[derive(Debug)]` required by `Result::unwrap()` in assertions
- **Fix:** Added `#[derive(Debug)]` to the test-only `MockError` struct
- **Files modified:** src/proxy/retry.rs (test module only)
- **Verification:** All 11 tests compile and pass
- **Committed in:** afa0585 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug in test code)
**Impact on plan:** Trivial test code fix. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Retry module is self-contained and fully tested
- Ready for Plan 03 handler integration: wire retry_with_fallback into chat_completions handler
- HasStatusCode trait needs to be implemented on RequestError in handlers.rs
- CandidateInfo needs conversion from SelectedProvider in the handler

---
*Phase: 04-retry-and-fallback*
*Completed: 2026-02-04*
