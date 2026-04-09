---
phase: 21-vault-billing-wiring
plan: "02"
subsystem: proxy/vault
tags: [integration-tests, vault-billing, mock-server]
dependency_graph:
  requires: [21-01]
  provides: [vault-billing-integration-tests]
  affects: [tests/vault_billing.rs, tests/common/mod.rs]
tech_stack:
  added: []
  patterns: [mock-http-server, timestamp-ordering-assertion, call-log-recording]
key_files:
  created:
    - tests/vault_billing.rs
  modified:
    - tests/common/mod.rs
decisions:
  - Used dual mock server pattern (vault + provider) on random ports for true end-to-end integration testing
  - Used Arc<Mutex<Vec<(String, Value, Instant)>>> for call recording with timestamps to verify ordering
  - Added setup_free_proxy_test_app helper for vault-absent test scenario
metrics:
  duration: 197s
  completed: "2026-04-09T18:59:37Z"
  tasks_completed: 1
  tasks_total: 1
  files_created: 1
  files_modified: 1
  tests_added: 9
---

# Phase 21 Plan 02: Vault Billing Integration Tests Summary

Mock vault and mock provider integration tests verifying end-to-end billing flow with timestamp ordering assertions

## What Was Done

### Task 1: Create mock vault server, mock provider, and vault billing integration tests

Added 9 integration tests in `tests/vault_billing.rs` using two mock HTTP servers (vault + provider) running on random ports. Added 3 new test helper functions to `tests/common/mod.rs`.

**Tests implemented:**
1. `test_vault_reserve_requires_bearer_token` - 401 without Authorization header
2. `test_vault_reserve_insufficient_balance` - 402 mapped to billing_error
3. `test_vault_reserve_policy_denied` - 403 mapped to billing_error
4. `test_vault_reserve_rate_limited` - 429 mapped to billing_error
5. `test_vault_reserve_uses_frontier_rates` - Verifies reserve amount uses frontier (worst-case) rates not local rates
6. `test_full_reserve_route_settle_path` - BILL-02 ordering + BILL-03 settle with metadata
7. `test_vault_release_on_provider_failure` - BILL-04 release on error
8. `test_free_proxy_mode_no_vault` - BILL-08 no auth required
9. `test_vault_auth_replaces_server_auth` - D-01 vault replaces server auth

**Test helpers added to common/mod.rs:**
- `setup_vault_test_app(vault_url, provider_url)` - App with vault billing enabled
- `setup_vault_test_app_with_auth(vault_url, provider_url, auth_token)` - With optional server auth
- `setup_free_proxy_test_app(provider_url)` - Free proxy mode (no vault)

**Commits:**
- `92b5ab7` - test(21-02): add vault billing integration tests with mock vault and provider

## Requirements Verified

| Requirement | Verification |
|-------------|-------------|
| BILL-02 | Reserve timestamp < provider contact timestamp (ordering assertion) |
| BILL-03 | Settle called with reservation_id, actual_msats, metadata after success |
| BILL-04 | Release called (not settle) when provider returns 500 |
| BILL-05 | Reserve amount >100k msats (frontier rates), not ~20k (local rates) |
| BILL-06 | Agent bearer token required, forwarded to vault |
| BILL-08 | Free proxy mode works without vault config or auth |

## Deviations from Plan

None - plan executed exactly as written.

## Self-Check: PASSED
