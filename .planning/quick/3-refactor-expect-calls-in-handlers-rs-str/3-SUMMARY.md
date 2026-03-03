---
phase: quick-3
plan: 01
subsystem: api
tags: [error-handling, panic-safety, handlers, retry]

# Dependency graph
requires: []
provides:
  - "Panic-free production request handlers in handlers.rs"
  - "Panic-free retry logic in retry.rs"
affects: [proxy]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "if-let-Ok guard pattern for infallible-in-practice HeaderValue creation"
    - "unwrap_or_else(|e| e.into_inner()) for poisoned mutex recovery"
    - "futures::future::Either for fallible closure returning Future"

key-files:
  created: []
  modified:
    - src/proxy/handlers.rs
    - src/proxy/retry.rs

key-decisions:
  - "Header value insertions use if-let-Ok with tracing::error fallback instead of .unwrap()"
  - "Retry closure uses futures::future::Either to return error Future on provider lookup miss"
  - "Trailing SSE serialization uses unwrap_or_else with empty string fallback (spawned task cannot propagate errors)"
  - "retry.rs last_error .expect() retained with SAFETY comment (provably unreachable code path)"
  - "assert! changed to debug_assert! in retry_with_fallback (checked in tests, skipped in release)"

patterns-established:
  - "Poisoned mutex recovery: .unwrap_or_else(|e| e.into_inner()) consistently applied"
  - "Fallible header insertion: if-let-Ok pattern with tracing::error on failure"

requirements-completed: [QUICK-3]

# Metrics
duration: 5min
completed: 2026-03-03
---

# Quick Task 3: Refactor expect/unwrap calls in handlers.rs and retry.rs

**Panic-free production handlers via if-let-Ok header guards, futures::Either fallible closures, and poisoned mutex recovery**

## Performance

- **Duration:** 5 min
- **Started:** 2026-03-03T02:55:18Z
- **Completed:** 2026-03-03T03:00:18Z
- **Tasks:** 3
- **Files modified:** 2

## Accomplishments
- Eliminated all .expect() and .unwrap() calls from production code in handlers.rs (10 sites refactored)
- Replaced assert! with debug_assert! and mutex locks with poisoned-mutex recovery in retry.rs
- All 194 tests pass, clippy clean, fmt clean

## Task Commits

Each task was committed atomically:

1. **Task 1: Replace .expect()/.unwrap() in handlers.rs** - `4b20c1b` (refactor)
2. **Task 2: Replace .expect()/.unwrap()/assert! in retry.rs** - `2a39aae` (refactor)
3. **Task 3: Full test suite and clippy validation** - `18d77ba` (chore - formatting fix)

## Files Created/Modified
- `src/proxy/handlers.rs` - All production .expect()/.unwrap() replaced with proper error handling
- `src/proxy/retry.rs` - assert! to debug_assert!, mutex recovery, SAFETY-documented expect

## Decisions Made
- Header values (request ID, cost, provider name, retries) use if-let-Ok guard pattern since they are derived from controlled inputs and practically infallible, but the guard prevents panics on unexpected values
- The retry closure that looks up a provider by name now uses futures::future::Either to return an error Future rather than panicking, because the closure must return a Future (not Result<Future>)
- The trailing SSE event serialization uses unwrap_or_else with empty string fallback because it runs inside a spawned task where errors cannot propagate via ?
- The retry.rs last_error .expect() is retained with a SAFETY comment because the code path is provably unreachable (the for loop always runs at least once and always sets last_error on Err)
- Response builder .unwrap() calls replaced with .map_err to RequestError, propagating as HTTP 500

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Production code in handlers.rs and retry.rs is now panic-safe
- stream.rs was already clean (all .expect()/.unwrap() in #[cfg(test)] only)

## Self-Check

Verified below.

---
*Phase: quick-3*
*Completed: 2026-03-03*
