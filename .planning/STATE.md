# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-16)

**Core value:** Smart model selection that minimizes sats spent per request without sacrificing quality
**Current focus:** Phase 15 -- Enhanced Health Endpoint (complete)

## Current Position

Phase: 15 of 15 (Enhanced Health Endpoint)
Plan: 1 of 1 in current phase
Status: Phase 15 complete -- all plans executed. Milestone v1.4 complete.
Last activity: 2026-02-16 -- Plan 15-01 executed (enhanced health endpoint with per-provider circuit state)

Progress: [██████████] 100% (5/5 plans)

## Performance Metrics

**v1 Velocity:**
- Total plans completed: 10
- Average duration: 2 min

**v1.1 Velocity:**
- Total plans completed: 4
- Average duration: 2.75 min

**v1.2 Velocity:**
- Total plans completed: 4
- Average duration: 3.5 min

**v1.3 Velocity:**
- Total plans completed: 4
- Average duration: 3.5 min

**v1.4 Velocity:**
- Total plans completed: 4
- Average duration: 4 min

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
See .planning/milestones/ for per-milestone decision history.

**v1.4 research decisions:**
- DashMap for per-provider circuit state (per-shard locking, no cross-provider contention)
- std::sync::Mutex (not tokio::sync::Mutex) -- no .await points in state transitions
- tokio::time::Instant for deterministic testing with start_paused/advance
- Lazy Open->HalfOpen transitions (checked on request, no background timer)
- Hardcoded constants (threshold=3, timeout=30s) -- defer configurability
- Handler-level integration (not router or middleware)
- Filter candidates BEFORE retry loop (prevents retry storm amplification)
- Single-permit half-open model (probe_in_flight flag prevents burst)

**v1.4 execution decisions (13-01):**
- Module-level #![allow(dead_code)] for clippy compliance until Plan 13-02 consumes types
- record_success logs at DEBUG level (routine operation, not state transition)

**v1.4 execution decisions (13-02):**
- watch::subscribe() stale-value prevention instead of immediate Pending reset after probe result
- Unknown providers allowed through acquire_permit (opt-in for configured providers)
- Empty CircuitBreakerRegistry for test AppState construction in existing integration tests

**v1.4 execution decisions (14-01):**
- ProbeGuard resolved before match timeout_result using &-references to avoid move conflicts
- is_circuit_failure (500-599 range) for recording failures, aligned with retry::is_retryable
- Probe candidate inserted at index 0 in filtered_candidates to become retry primary

**v1.4 execution decisions (14-02):**
- Removed execute_request function -- streaming path uses select_candidates + send_to_provider directly
- ProbeGuard created before send_to_provider and resolved via match &result pattern
- Integration tests use lightweight axum mock servers on random ports (not wiremock)

**v1.4 execution decisions (15-01):**
- HealthResponse has only status and providers fields (no service field) per locked decision
- all_states() uses DashMap per-shard locks, no global lock for snapshot collection
- Half-open counts as degraded (not unhealthy) -- only fully Open circuits trigger unhealthy

### Pending Todos

None.

### Blockers/Concerns

- Routstr provider stream_options support unknown -- safe degradation (NULL usage) prevents regression

## Session Continuity

Last session: 2026-02-16
Stopped at: Completed 15-01-PLAN.md (enhanced health endpoint with per-provider circuit state)
Resume file: Phase 15 complete. Milestone v1.4 (Circuit Breaker) complete.
