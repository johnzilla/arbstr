---
phase: 21-vault-billing-wiring
verified: 2026-04-09T19:11:53Z
status: passed
score: 5/5 must-haves verified
overrides_applied: 0
---

# Phase 21: Vault Billing Wiring Verification Report

**Phase Goal:** Every inference request is billed through arbstr vault -- reserve before routing, settle on success, release on failure
**Verified:** 2026-04-09T19:11:53Z
**Status:** passed
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Sending a chat completion with a valid agent token reserves funds, routes inference, and settles actual cost to vault | VERIFIED | `test_full_reserve_route_settle_path` passes: reserve called before provider, settle called with reservation_id + actual_msats + metadata after success |
| 2 | A failed inference request releases the reservation back to the buyer's account (no funds lost) | VERIFIED | `test_vault_release_on_provider_failure` passes: mock provider returns 500, release endpoint called (not settle) |
| 3 | Reserve amount uses frontier-tier pricing regardless of scored tier, so tier escalation never under-reserves | VERIFIED | `Router::frontier_rates()` in selector.rs returns max rates across all providers; handlers.rs line 627-635 calls `frontier_rates()` before reserve; `test_vault_reserve_uses_frontier_rates` asserts reserve amount >100k msats (frontier) vs ~20k (local) |
| 4 | When vault config is absent, proxy operates identically to pre-v2.0 behavior (free proxy mode) | VERIFIED | `test_free_proxy_mode_no_vault` passes: no auth required, request succeeds with provider response |
| 5 | Agent bearer token from Authorization header is forwarded to vault and replaces server-level auth_token for proxy endpoints | VERIFIED | handlers.rs lines 584-616 extract Bearer token, line 646 forwards to `vault.reserve(agent_token, ...)`; server.rs lines 101-114 skip auth middleware when vault is configured; `test_vault_auth_replaces_server_auth` passes |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/router/selector.rs` | frontier_rates method for worst-case cost lookup | VERIFIED | `pub fn frontier_rates(&self, model: &str) -> Option<(u64, u64, u64)>` at line 216; 3 unit tests present |
| `src/proxy/handlers.rs` | Reserve call using frontier rates instead of cheapest candidate | VERIFIED | Lines 623-641: comment "Reserve at frontier (worst-case) rates per D-03/D-04", calls `frontier_rates(&ctx.model)` with fallback |
| `src/proxy/vault.rs` | Unit test for frontier-rate estimation | VERIFIED | `test_estimate_reserve_frontier_rates` at line 592 verifies 125880 msats calculation |
| `tests/vault_billing.rs` | Integration tests with mock HTTP vault and mock provider | VERIFIED | 9 tests; `start_mock_vault` + `start_mock_provider` helpers present; all tests pass |
| `tests/common/mod.rs` | Updated test helpers with vault-enabled app setup | VERIFIED | `setup_vault_test_app`, `setup_vault_test_app_with_auth`, `setup_free_proxy_test_app` all present |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `src/proxy/handlers.rs` | `src/router/selector.rs` | `frontier_rates()` call for reserve estimation | WIRED | Line 629: `state.router.frontier_rates(&ctx.model)` |
| `src/proxy/handlers.rs` | `src/proxy/vault.rs` | `estimate_reserve_msats` with frontier rates | WIRED | Line 636: `super::vault::estimate_reserve_msats(est_input, est_output, reserve_input_rate, ...)` |
| `tests/vault_billing.rs` | `src/proxy/handlers.rs` | HTTP requests to `/v1/chat/completions` with vault configured | WIRED | All 9 tests post to `/v1/chat/completions` via `setup_vault_test_app` |
| `tests/vault_billing.rs` | `src/proxy/vault.rs` | Mock vault verifies reserve/settle/release calls | WIRED | Mock captures `/internal/reserve`, `/internal/settle`, `/internal/release` calls with timestamps |

### Data-Flow Trace (Level 4)

Not applicable -- this phase modifies routing/billing logic (not data-rendering components). The key data flow is the reserve-route-settle call chain, which is verified end-to-end by integration tests rather than static data-flow trace.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| All vault billing integration tests pass | `cargo test --test vault_billing` | 9 passed; 0 failed; finished in 3.25s | PASS |
| All unit tests pass (no regressions) | `cargo test` | 168 unit tests passed; 0 failed | PASS |
| Clippy clean | `cargo clippy -- -D warnings` | Finished dev profile, 0 warnings | PASS |
| frontier_rates unit tests cover 3 cases | grep test names in selector.rs | `test_frontier_rates_returns_max_across_tiers`, `test_frontier_rates_single_tier_returns_that_tier`, `test_frontier_rates_nonexistent_model_returns_none` all present | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|---------|
| BILL-01 | 21-01 | Extract agent bearer token from Authorization header and forward to vault reserve | SATISFIED | handlers.rs lines 584-616 extract Bearer token; line 646 forwards to `vault.reserve(agent_token, ...)` |
| BILL-02 | 21-01, 21-02 | Reserve funds BEFORE routing inference | SATISFIED | `test_full_reserve_route_settle_path` asserts `reserve_call.2 < provider_call.2` (Instant timestamps) |
| BILL-03 | 21-02 | Successful inference settles actual cost with token/provider/latency metadata | SATISFIED | `test_full_reserve_route_settle_path` asserts settle called with `reservation_id`, `actual_msats`, `metadata.provider`, `metadata.latency_ms` |
| BILL-04 | 21-02 | Failed inference releases reservation | SATISFIED | `test_vault_release_on_provider_failure` asserts release called (not settle) when provider returns 500 |
| BILL-05 | 21-01, 21-02 | Reserve uses worst-case (frontier-tier) pricing | SATISFIED | `Router::frontier_rates()` method; handlers.rs uses it for reserve; `test_vault_reserve_uses_frontier_rates` verifies amount >100k msats |
| BILL-06 | 21-01, 21-02 | Vault agent token replaces server-level auth_token when vault configured | SATISFIED | server.rs lines 101-114 skip auth middleware when `has_vault`; `test_vault_auth_replaces_server_auth` passes |
| BILL-07 | (Phase 22) | Pending settlements persist and replay on restart | NOT IN SCOPE | Correctly assigned to Phase 22 per REQUIREMENTS.md traceability table |
| BILL-08 | 21-02 | Vault billing skipped when vault config absent (free proxy mode) | SATISFIED | `test_free_proxy_mode_no_vault` passes: no auth required, inference succeeds |

All 7 Phase 21 requirement IDs (BILL-01 through BILL-06, BILL-08) are SATISFIED. BILL-07 is correctly deferred to Phase 22 and does not appear in either plan's `requirements` field.

### Anti-Patterns Found

None. Scanned all 5 modified/created files for TODO/FIXME/PLACEHOLDER/empty implementations. No anti-patterns detected.

### Human Verification Required

None. All must-haves verified programmatically through integration test execution.

### Gaps Summary

No gaps. All 5 observable truths verified, all artifacts exist with substantive implementation and correct wiring, all 7 requirement IDs satisfied by integration tests that pass in the actual test runner.

---

_Verified: 2026-04-09T19:11:53Z_
_Verifier: Claude (gsd-verifier)_
