---
phase: 18-tier-aware-routing
plan: 01
subsystem: router
tags: [tier, routing, complexity, provider-selection]

requires:
  - phase: 17-complexity-scorer
    provides: complexity scoring function (score_complexity)
provides:
  - score_to_max_tier mapping function (score -> Tier)
  - Tier-filtered select_candidates with max_tier parameter
  - Backward-compatible select/select_candidates with None default
affects: [19-handler-integration]

tech-stack:
  added: []
  patterns: [tier-filtering-predicate, optional-parameter-backward-compat]

key-files:
  created: []
  modified:
    - src/router/complexity.rs
    - src/router/selector.rs
    - src/router/mod.rs
    - src/proxy/handlers.rs

key-decisions:
  - "Tier filter placed after model filter but before policy constraints in select_candidates pipeline"
  - "score_to_max_tier is a free function taking explicit threshold params rather than reading config directly"

patterns-established:
  - "Optional parameter pattern: new routing parameters added as Option<T> with None preserving existing behavior"
  - "Tier comparison uses derived Ord: provider.tier <= max_tier includes all tiers up to and including max_tier"

requirements-completed: [ROUTE-01, ROUTE-02, ROUTE-03]

duration: 3min
completed: 2026-04-08
---

# Phase 18 Plan 01: Tier-Aware Routing Summary

**score_to_max_tier mapping and tier-filtered provider selection via max_tier parameter on select_candidates/select**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-08T23:44:13Z
- **Completed:** 2026-04-08T23:47:08Z
- **Tasks:** 2
- **Files modified:** 4

## Accomplishments
- Added score_to_max_tier function mapping complexity scores to Tier::Local/Standard/Frontier based on configurable thresholds
- Extended select_candidates and select with Option<Tier> max_tier parameter for tier-based provider filtering
- All 161 existing tests pass unchanged with None parameter (full backward compatibility)
- 8 new tests: 3 boundary condition tests for score_to_max_tier, 5 tier filtering tests for select_candidates

## Task Commits

Each task was committed atomically:

1. **Task 1: Add score_to_max_tier mapping function** - `fff4d78` (feat)
2. **Task 2: Add max_tier parameter to select_candidates and select** - `02fb39a` (feat)

## Files Created/Modified
- `src/router/complexity.rs` - Added score_to_max_tier function and 3 boundary tests
- `src/router/mod.rs` - Re-exported score_to_max_tier
- `src/router/selector.rs` - Added max_tier parameter to select/select_candidates, tier filter predicate, 5 new tests
- `src/proxy/handlers.rs` - Updated both caller sites to pass None for max_tier

## Decisions Made
- Tier filter placed after model filter but before policy constraints -- this ensures model availability is checked first, then tier narrows candidates before cost constraints are applied
- score_to_max_tier takes explicit (score, low, high) parameters rather than a config struct -- keeps the function pure and testable without config dependency

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Updated second select caller in handlers.rs**
- **Found during:** Task 2
- **Issue:** Plan mentioned only one select_candidates call site (line ~248) but handlers.rs also has a select() call at line 1447 (cost endpoint)
- **Fix:** Added None as fourth argument to the cost endpoint's select() call
- **Files modified:** src/proxy/handlers.rs
- **Verification:** cargo test passes, cargo clippy clean
- **Committed in:** 02fb39a (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Necessary fix to avoid compilation error. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- score_to_max_tier and tier-filtered select_candidates ready for Phase 19 handler integration
- Phase 19 will wire complexity scoring into the request path and pass computed max_tier to select_candidates

## Self-Check: PASSED

- All 4 modified files exist on disk
- Both task commits (fff4d78, 02fb39a) verified in git log
- score_to_max_tier function present and re-exported
- max_tier parameter on select_candidates/select confirmed
- Tier predicate (p.tier <= max_tier) confirmed
- Handler callers pass None at lines 248 and 1447
- cargo test: all 161 tests pass
- cargo clippy: clean (no warnings)

---
*Phase: 18-tier-aware-routing*
*Completed: 2026-04-08*
