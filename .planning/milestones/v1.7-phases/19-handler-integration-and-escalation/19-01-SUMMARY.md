---
phase: 19-handler-integration-and-escalation
plan: 01
subsystem: proxy/handlers, router/selector, config
tags: [escalation, complexity-routing, tier-override, circuit-breaker-escalation]
dependency_graph:
  requires: [17-01-complexity-scorer, 18-01-tier-aware-routing]
  provides: [resolve_candidates-scoring, tier-escalation, complexity-header-override]
  affects: [proxy/handlers.rs, router/selector.rs, config.rs, error.rs]
tech_stack:
  added: []
  patterns: [one-way-escalation-loop, header-tier-override, combined-select-circuit-escalation]
key_files:
  created:
    - tests/escalation.rs
  modified:
    - src/proxy/handlers.rs
    - src/config.rs
    - src/error.rs
    - src/router/selector.rs
decisions:
  - "Escalation loop wraps both select_candidates AND circuit breaker filtering (Pitfall 2 from research)"
  - "NoTierMatch error variant distinguishes tier-empty from policy-empty results"
  - "Complexity header parsed in chat_completions, passed as Option<Tier> to resolve_candidates"
  - "ResolvedCandidates extended with complexity_score and tier for Phase 20 response headers"
metrics:
  duration_seconds: 330
  completed: "2026-04-09T01:40:10Z"
  tasks_completed: 3
  tasks_total: 3
  files_modified: 5
  tests_added: 10
  tests_total_after: "174+"
---

# Phase 19 Plan 01: Handler Integration and Escalation Summary

End-to-end complexity scoring and tier-aware routing wired into resolve_candidates with X-Arbstr-Complexity header override and one-way tier escalation through circuit breaker filtering.

## What Was Done

### Task 1: Tier::escalate, NoTierMatch error, selector fix
- Added `Tier::escalate()` method: Local->Standard->Frontier->None (one-way)
- Added `Error::NoTierMatch { tier, model }` variant mapped to 400 BAD_REQUEST
- Changed selector tier-filter empty path from `NoPolicyMatch` to `NoTierMatch`
- Updated `routing_error_status` to handle `NoTierMatch`
- Added 3 unit tests for escalate behavior
- **Commit:** `1ef1521`

### Task 2: Wire scoring, header override, and escalation into resolve_candidates
- Added `ARBSTR_COMPLEXITY_HEADER` constant (`x-arbstr-complexity`)
- Added imports for `score_complexity`, `score_to_max_tier`, `Tier`
- Extended `ResolvedCandidates` with `complexity_score: Option<f64>` and `tier: Tier`
- Changed `resolve_candidates` signature to accept `messages` and `complexity_override`
- Implemented header parsing: high->Frontier, medium->Standard, low->Local, invalid->None
- Implemented scoring path: calls `score_complexity` + `score_to_max_tier` when no override
- Implemented escalation loop wrapping both `select_candidates` AND circuit breaker filtering
- Escalation triggers on NoTierMatch (no configured providers) and empty circuit-filtered list
- **Commit:** `290f736`

### Task 3: Integration tests for header override and escalation
- 7 integration tests covering SCORE-03, ROUTE-04, ROUTE-05
- Tests use tiered mock providers (local, standard, frontier) with real HTTP servers
- Header override tests: high, low, medium, invalid, missing
- Escalation tests: single circuit-broken tier, double escalation (local+standard broken)
- **Commit:** `659f326`

## Deviations from Plan

None -- plan executed exactly as written.

## Decisions Made

1. **Escalation loop includes circuit breaker filtering** -- Research Pitfall 2 identified that circuit-broken providers at a tier should trigger escalation. The loop wraps both `select_candidates` and circuit breaker filtering, preventing 503s when higher-tier providers are available.

2. **NoTierMatch as distinct error variant** -- Distinguishes "no providers at this tier" from "no providers match policy constraints." This prevents wasteful escalation on policy mismatches (higher tiers are more expensive, not less constrained).

3. **Header parsed outside resolve_candidates** -- Following the existing pattern where `policy_name` is extracted in `chat_completions` and passed to resolve_candidates. Keeps resolve_candidates focused on routing logic.

4. **ResolvedCandidates extended for Phase 20** -- Added `complexity_score` and `tier` fields now to avoid re-scoring in Phase 20 (response headers/SSE metadata). Fields are `#[allow(dead_code)]` until consumed.

## Verification Results

- `cargo test --lib` -- 164 passed, 0 failed
- `cargo test --test escalation` -- 7 passed, 0 failed
- `cargo test` -- full suite green, no regressions
- `cargo clippy -- -D warnings` -- clean, no warnings
