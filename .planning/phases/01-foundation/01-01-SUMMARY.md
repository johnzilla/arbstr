---
phase: 01-foundation
plan: 01
subsystem: router
tags: [cost-calculation, routing, tdd, satoshis]

# Dependency graph
requires:
  - phase: none
    provides: existing Router and ProviderConfig with output_rate and base_fee fields
provides:
  - Corrected select_cheapest ranking using output_rate + base_fee
  - Public actual_cost_sats function for post-response cost calculation
affects: [02-request-logging, 03-response-metadata]

# Tech tracking
tech-stack:
  added: []
  patterns: [TDD red-green-refactor, f64 cost precision for sub-sat amounts]

key-files:
  created: []
  modified:
    - src/router/selector.rs
    - src/router/mod.rs

key-decisions:
  - "Routing heuristic uses output_rate + base_fee (not full formula) since token counts are unknown at selection time"
  - "actual_cost_sats returns f64 to preserve sub-satoshi precision for cheap models"
  - "Cast to f64 before division to avoid integer truncation"

patterns-established:
  - "Cost functions use f64 return type for sub-sat precision"
  - "Routing heuristic approximates with available data; actual cost computed post-response"

# Metrics
duration: 3min
completed: 2026-02-02
---

# Phase 1 Plan 1: Fix Cost Ranking and Add Actual Cost Function Summary

**Corrected select_cheapest to rank by output_rate+base_fee and added actual_cost_sats(f64) for post-response cost logging**

## Performance

- **Duration:** 2 min 37 sec
- **Started:** 2026-02-03T03:40:31Z
- **Completed:** 2026-02-03T03:43:08Z
- **Tasks:** 1 TDD feature (RED + GREEN, no REFACTOR needed)
- **Files modified:** 2

## Accomplishments
- Fixed cost ranking bug: providers with high base_fee but low output_rate are no longer incorrectly chosen as cheapest
- Added public `actual_cost_sats` function with full formula: `(input_tokens * input_rate + output_tokens * output_rate) / 1000.0 + base_fee`
- Sub-satoshi precision preserved via f64 (0.125 sats, not 0)
- 3 new tests covering ranking, cost calculation (4 cases), and fractional precision
- All 8 tests pass (5 existing + 3 new), zero regressions

## Task Commits

Each task was committed atomically:

1. **RED: Failing tests** - `4953111` (test)
   - test_base_fee_affects_cheapest_selection
   - test_actual_cost_calculation (4 cases)
   - test_actual_cost_fractional_sats
2. **GREEN: Implementation** - `b83601e` (feat)
   - select_cheapest ranking fix
   - actual_cost_sats function
   - Re-export from router module

_TDD plan: 2 commits (test + feat). No refactoring needed._

## Files Created/Modified
- `src/router/selector.rs` - Fixed select_cheapest ranking key, added actual_cost_sats function and 3 tests
- `src/router/mod.rs` - Re-exported actual_cost_sats for Phase 2 consumption

## Decisions Made
- **Routing heuristic uses output_rate + base_fee only**: At selection time, token counts are unknown, so the full formula cannot be used. The heuristic `output_rate + base_fee` is a reasonable proxy since output tokens typically dominate cost.
- **actual_cost_sats returns f64**: Integer division would truncate sub-satoshi values (e.g., 0.125 sats becomes 0). Preserving precision matters for cheap models with small token counts and for accurate cost aggregation over many requests.
- **Re-exported actual_cost_sats from router module**: The function needs to be callable from Phase 2's logging code, so it must be publicly accessible through the module hierarchy.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Re-exported actual_cost_sats from router/mod.rs**
- **Found during:** GREEN phase (implementation)
- **Issue:** `actual_cost_sats` was `pub` in `selector.rs` but not re-exported from `router/mod.rs`, causing a dead_code warning and making it inaccessible to Phase 2
- **Fix:** Added `actual_cost_sats` to the `pub use selector::{...}` line in `mod.rs`
- **Files modified:** `src/router/mod.rs`
- **Verification:** Warning eliminated, function accessible as `arbstr::router::actual_cost_sats`
- **Committed in:** `b83601e` (part of GREEN commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Necessary for the function to be usable by Phase 2. No scope creep.

## Issues Encountered
- Pre-existing dirty working tree included `src/proxy/server.rs` changes (from plan 01-02 work). Initial GREEN commit accidentally included it; fixed by resetting and re-committing with only the router files.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- `actual_cost_sats` is ready for Phase 2 request logging to compute real costs from provider response usage data
- `select_cheapest` now correctly accounts for base_fee, ensuring logged costs reflect the truly cheapest provider
- Plan 01-02 (correlation IDs) has no dependency on this plan and can proceed independently

---
*Phase: 01-foundation*
*Completed: 2026-02-02*
