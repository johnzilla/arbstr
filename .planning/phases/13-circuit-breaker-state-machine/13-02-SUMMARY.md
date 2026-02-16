---
phase: 13-circuit-breaker-state-machine
plan: 02
subsystem: proxy
tags: [circuit-breaker, dashmap, tokio-watch, raii, concurrency, queue-and-wait]

# Dependency graph
requires:
  - phase: 13-01
    provides: CircuitBreakerInner state machine with Closed/Open/HalfOpen lifecycle
provides:
  - CircuitBreakerRegistry with DashMap-backed per-provider circuit breakers
  - acquire_permit with queue-and-wait semantics via tokio::sync::watch
  - PermitType enum (Normal/Probe) for caller dispatch
  - ProbeGuard RAII for stuck-probe prevention
  - Read-only accessors (state, failure_count, trip_count) for health endpoint
  - CircuitBreakerRegistry in AppState via Arc for shared access
  - Public re-exports in proxy/mod.rs for downstream consumption
affects: [14-routing-integration, 15-health-endpoint]

# Tech tracking
tech-stack:
  added: []
  patterns: [dashmap-registry, tokio-watch-broadcast, raii-guard-probe, queue-and-wait-half-open]

key-files:
  created: []
  modified: [src/proxy/circuit_breaker.rs, src/proxy/mod.rs, src/proxy/server.rs, tests/stats.rs, tests/logs.rs]

key-decisions:
  - "watch::subscribe() stale-value prevention instead of immediate Pending reset after probe result"
  - "Unknown providers allowed through acquire_permit (circuit breaker is opt-in for configured providers)"
  - "Empty registry for test AppState construction (no circuit breakers needed for existing tests)"

patterns-established:
  - "CircuitBreakerRegistry::new(&provider_names) for Arc-wrapped shared state"
  - "ProbeGuard RAII pattern: success()/failure()/drop-as-failure for probe lifecycle"
  - "DashMap entry + Mutex lock scope minimization: extract data, drop all locks, then await"

requirements-completed: [CB-01, CB-04, CB-05, CB-06]

# Metrics
duration: 6min
completed: 2026-02-16
---

# Phase 13 Plan 02: Circuit Breaker Registry and Concurrency Layer Summary

**DashMap-backed per-provider circuit breaker registry with tokio::sync::watch queue-and-wait probing, ProbeGuard RAII, and AppState integration**

## Performance

- **Duration:** 6 min
- **Started:** 2026-02-16T20:26:34Z
- **Completed:** 2026-02-16T20:33:32Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- CircuitBreakerRegistry with DashMap for per-provider concurrent access, acquire_permit with full queue-and-wait semantics
- ProbeGuard RAII type ensuring probe_in_flight flag is always cleared (success, failure, or drop)
- 13 new registry/concurrency tests including 5-waiter concurrency test and stale probe result prevention
- Registry wired into AppState with Arc for Phase 14/15 consumption; all 166 tests pass

## Task Commits

Each task was committed atomically:

1. **Task 1: Registry, ProviderCircuitBreaker, acquire_permit, and ProbeGuard** - `d08f8d3` (feat)
2. **Task 2: Wire registry into AppState and add module re-exports** - `1c7de6c` (feat)

## Files Created/Modified
- `src/proxy/circuit_breaker.rs` - Extended from 528 to 1055 lines: added PermitType, ProviderCircuitBreaker, CircuitBreakerRegistry, ProbeGuard, 13 new tests
- `src/proxy/mod.rs` - Added pub use re-exports for CircuitBreakerRegistry, CircuitOpenError, CircuitState, PermitType, ProbeGuard
- `src/proxy/server.rs` - Added circuit_breakers field to AppState, initialized registry in run_server
- `tests/stats.rs` - Updated AppState construction with empty CircuitBreakerRegistry
- `tests/logs.rs` - Updated AppState construction with empty CircuitBreakerRegistry

## Decisions Made
- **watch::subscribe() for stale value prevention**: Instead of immediately resetting the watch channel to Pending after sending Success/Failed (which caused a race where waiters saw Pending instead of the result), rely on subscribe() semantics where new subscribers mark the current value as "seen" and only wake on subsequent sends. This naturally prevents stale values from previous probe cycles.
- **Unknown providers allowed through**: acquire_permit returns Ok(Normal) for provider names not in the registry, making circuit breakers opt-in for configured providers only.
- **Empty registry for test AppState**: Existing integration tests (stats, logs) use `CircuitBreakerRegistry::new(&[])` since they don't exercise circuit breakers.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed watch channel reset causing waiter deadlock**
- **Found during:** Task 1 (queue-and-wait tests)
- **Issue:** Plan specified "send ProbeResult::Pending immediately after Success/Failed to reset for next cycle." This caused a race: the Pending value overwrote Success/Failed before waiters could read it via borrow(), causing them to loop forever on the Pending continue branch.
- **Fix:** Removed immediate Pending reset. Relied on watch::subscribe() semantics instead -- new subscribers mark the current value as "seen", so stale Success/Failed from previous cycles are invisible to new waiters.
- **Files modified:** src/proxy/circuit_breaker.rs
- **Verification:** All 4 queue-and-wait tests pass (success, failure, multiple waiters, stale prevention)
- **Committed in:** d08f8d3 (part of Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug)
**Impact on plan:** Critical correctness fix. The plan's watch channel reset strategy had a TOCTOU race. The fix uses a simpler and more correct approach. No scope creep.

## Issues Encountered
None beyond the deviation documented above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- CircuitBreakerRegistry is fully functional and accessible via `AppState.circuit_breakers`
- Phase 14 (routing integration) can call acquire_permit before sending requests and record_success/failure after
- Phase 15 (health endpoint) can use state()/failure_count()/trip_count() read-only accessors
- ProbeGuard is ready for Phase 14's handler-level probe lifecycle management
- All types are re-exported from `crate::proxy` for ergonomic imports
- 29 circuit breaker tests (16 state machine + 13 registry) validate correctness
- cargo clippy -- -D warnings passes clean

## Self-Check: PASSED

- [x] src/proxy/circuit_breaker.rs exists (1055 lines, min 350)
- [x] src/proxy/circuit_breaker.rs contains CircuitBreakerRegistry
- [x] src/proxy/mod.rs contains circuit_breaker re-exports
- [x] src/proxy/server.rs contains CircuitBreakerRegistry
- [x] Commit d08f8d3 exists (Task 1: registry + ProbeGuard)
- [x] Commit 1c7de6c exists (Task 2: AppState wiring)
- [x] 29 circuit breaker tests pass
- [x] 166 total tests pass (no regressions)
- [x] cargo clippy -- -D warnings clean

---
*Phase: 13-circuit-breaker-state-machine*
*Completed: 2026-02-16*
