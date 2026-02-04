---
phase: 04-retry-and-fallback
plan: 01
subsystem: router
tags: [routing, provider-selection, fallback, candidate-list]

# Dependency graph
requires:
  - phase: 01-foundation
    provides: Router struct with select() method and SelectedProvider type
provides:
  - Router::select_candidates() returning Vec<SelectedProvider> sorted cheapest-first
  - Provider deduplication by name in candidate list
  - select() delegates to select_candidates() preserving existing behavior
affects: [04-02 retry-and-fallback-loop, 04-03 handler-integration]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Candidate list pattern: select_candidates returns ordered Vec, select delegates to it"
    - "HashSet dedup: deduplicate providers by name, keeping cheapest entry"

key-files:
  created: []
  modified:
    - src/router/selector.rs

key-decisions:
  - "Removed select_cheapest/select_first private methods since select_candidates handles sorting directly"
  - "default_strategy field retained with allow(dead_code) for future strategy dispatch"

patterns-established:
  - "select() delegates to select_candidates().remove(0) for single-provider selection"
  - "Candidate list sorted by routing cost (output_rate + base_fee) ascending"

# Metrics
duration: 2min
completed: 2026-02-04
---

# Phase 4 Plan 1: Candidate List Summary

**Router::select_candidates() returning ordered, deduplicated Vec<SelectedProvider> sorted by routing cost for retry/fallback provider selection**

## Performance

- **Duration:** 2 min
- **Started:** 2026-02-04T12:58:19Z
- **Completed:** 2026-02-04T13:00:08Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- Added `select_candidates` method to Router returning `Vec<SelectedProvider>` sorted cheapest-first by `output_rate + base_fee`
- Candidates deduplicated by provider name using `HashSet`, keeping the cheapest entry for each name
- Refactored existing `select()` to delegate to `select_candidates().remove(0)`, preserving identical behavior
- Removed dead code (`select_cheapest`, `select_first`) that was superseded by the new sorting approach
- Added 5 new unit tests covering ordering, dedup, delegation, model filtering, and error cases

## Task Commits

Each task was committed atomically:

1. **Task 1: Add select_candidates and refactor select** - `66e15ab` (feat)

## Files Created/Modified
- `src/router/selector.rs` - Added `select_candidates()` method, refactored `select()` to delegate, removed dead helper methods, added 5 unit tests

## Decisions Made
- Removed `select_cheapest` and `select_first` private methods since `select_candidates` now handles sorting directly via `sort_by_key` -- these methods were dead code after the refactor
- Retained `default_strategy` field with `#[allow(dead_code)]` annotation since it is part of the Router constructor API and will be needed for future strategy-based dispatch (lowest_latency, round_robin)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Removed dead code to satisfy clippy -D warnings**
- **Found during:** Task 1 (verification)
- **Issue:** `select_cheapest` and `select_first` methods became dead code after refactoring `select()` to delegate to `select_candidates()`, causing clippy warnings
- **Fix:** Removed both dead methods; added `#[allow(dead_code)]` on `default_strategy` field since it is a config-driven constructor parameter preserved for future use
- **Files modified:** src/router/selector.rs
- **Verification:** `cargo clippy -- -D warnings` passes clean
- **Committed in:** 66e15ab (part of task commit)

---

**Total deviations:** 1 auto-fixed (1 bug/cleanup)
**Impact on plan:** Dead code removal necessary for clippy compliance. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- `select_candidates()` API ready for Plan 02 (retry/fallback loop) to consume
- Returns ordered candidate list enabling primary + fallback provider selection
- All 22 project tests pass (11 router + 11 other)

---
*Phase: 04-retry-and-fallback*
*Completed: 2026-02-04*
