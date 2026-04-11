---
phase: 21-vault-billing-wiring
plan: 01
subsystem: router, proxy
tags: [vault, billing, reserve, frontier-rates, tdd]
dependency_graph:
  requires: []
  provides: [frontier_rates, frontier-reserve-pricing]
  affects: [vault-billing, tier-escalation]
tech_stack:
  added: []
  patterns: [worst-case-reserve-estimation]
key_files:
  created: []
  modified:
    - src/router/selector.rs
    - src/proxy/handlers.rs
    - src/proxy/vault.rs
decisions:
  - "Use independent max across input_rate, output_rate, base_fee for frontier rates (not tied to a single provider)"
metrics:
  duration: 135s
  completed: "2026-04-09"
  tasks: 1
  files_modified: 3
---

# Phase 21 Plan 01: Fix Vault Reserve Pricing Summary

Vault reserve estimation now uses frontier-tier (worst-case) rates instead of cheapest candidate rates, preventing under-reservation when tier escalation occurs on circuit break.

## What Changed

### Task 1: Add frontier_rates to Router and fix reserve pricing in handlers (TDD)

**RED:** Added 4 failing tests:
- `test_frontier_rates_returns_max_across_tiers` -- verifies max rates across local/standard/frontier
- `test_frontier_rates_single_tier_returns_that_tier` -- verifies single-tier fallback
- `test_frontier_rates_nonexistent_model_returns_none` -- verifies None for unknown models
- `test_estimate_reserve_frontier_rates` -- verifies 125880 msats calculation with frontier rates

**GREEN:** Implemented `Router::frontier_rates()` and updated `handlers.rs`:
- `frontier_rates(&self, model: &str) -> Option<(u64, u64, u64)>` takes independent max of input_rate, output_rate, base_fee across all providers serving the model
- `handlers.rs` reserve block now calls `state.router.frontier_rates(&ctx.model)` with fallback to cheapest candidate
- Comment documents D-03/D-04 rationale

**Commits:** `5adeb93` (RED), `e3e9cd3` (GREEN)

## Verification

- `cargo test` -- 248 tests passed (168 unit + 80 integration), 0 failures
- `cargo clippy -- -D warnings` -- clean, no warnings

## Deviations from Plan

None -- plan executed exactly as written.

## Decisions Made

1. **Independent max for frontier rates**: Rather than selecting a single "most expensive" provider, `frontier_rates` takes the max of each rate dimension independently. This provides true worst-case coverage even when no single provider is the most expensive in all dimensions.

## Threat Model Verification

- T-21-01 (Tampering): Mitigated -- reserve estimation now uses max rates across all providers, not client-controllable cheapest candidate
- T-21-02 (Elevation): Accepted -- frontier_rates only reads immutable provider config

## Self-Check: PASSED

- All 3 modified files exist
- Both commits (5adeb93, e3e9cd3) verified in git log
- All 5 acceptance criteria confirmed in source
