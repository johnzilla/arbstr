---
phase: 22-vault-fault-tolerance
plan: "01"
subsystem: proxy/vault
tags: [vault, fault-tolerance, reconciliation, eviction, testing]
dependency_graph:
  requires: []
  provides: [vault-eviction, vault-reconciliation-tests]
  affects: [src/proxy/vault.rs, tests/vault_fault_tolerance.rs, tests/common/mod.rs]
tech_stack:
  added: []
  patterns: [max-attempts-eviction, direct-db-test-pattern]
key_files:
  created:
    - tests/vault_fault_tolerance.rs
  modified:
    - src/proxy/vault.rs
    - tests/common/mod.rs
decisions:
  - Hardcoded MAX_SETTLEMENT_ATTEMPTS=10 constant instead of configurable -- simplicity, can add config later
  - Made reconcile_once, fetch_pending, delete_pending public for deterministic integration testing
  - reconcile_once returns (replayed, failed, evicted) tuple for test assertions
metrics:
  duration: 234s
  completed: "2026-04-09T20:55:50Z"
  tasks: 2
  files: 3
---

# Phase 22 Plan 01: Vault Fault Tolerance Summary

Eviction logic for stale pending settlements with comprehensive reconciliation tests covering direct DB insertion and full-cycle crash recovery.

## What Was Done

### Task 1: Eviction logic in reconcile_once

Modified `src/proxy/vault.rs` to add max-attempts eviction:

- Added `MAX_SETTLEMENT_ATTEMPTS` constant (10) for stale settlement threshold
- Modified `fetch_pending` to return attempts count as third tuple element
- Added eviction check in `reconcile_once` loop: settlements with `attempts >= 10` are deleted with `tracing::error!` logging (reservation_id, settlement_type, attempts) and skipped without HTTP replay
- Made `reconcile_once`, `fetch_pending`, `delete_pending` public for integration testing
- Changed `reconcile_once` return type to `(u32, u32, u32)` for `(replayed, failed, evicted)` stats
- Added `evicted` counter to reconciliation pass log message

### Task 2: Vault fault tolerance tests

Created `tests/vault_fault_tolerance.rs` with 5 tests:

1. **test_reconcile_replays_pending_settlements** -- 3 settlements (2 settle, 1 release) all replayed and deleted, vault receives correct calls
2. **test_reconcile_increments_attempts_on_failure** -- vault returns 500, row preserved with attempts=1
3. **test_reconcile_evicts_after_max_attempts** -- settlement with attempts=10 evicted without HTTP call to vault
4. **test_reconcile_mixed_eviction_and_replay** -- attempts=9 replayed normally, attempts=10 evicted, both deleted
5. **test_full_cycle_settle_failure_and_reconciliation** -- request -> settle 504 -> pending row persisted -> new vault -> reconcile -> settled

Added `setup_test_db()` helper to `tests/common/mod.rs` for in-memory SQLite with migrations.

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| 1 | cca93e0 | feat(22-01): add eviction logic to reconcile_once and expose for testing |
| 2 | 85249dd | test(22-01): add vault fault tolerance tests with direct DB and full-cycle coverage |

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo build` -- clean, no warnings
- `cargo test --test vault_fault_tolerance` -- 5/5 tests pass
- `cargo test` -- full suite passes (no regressions)
- `cargo clippy -- -D warnings` -- clean

## Self-Check: PASSED
