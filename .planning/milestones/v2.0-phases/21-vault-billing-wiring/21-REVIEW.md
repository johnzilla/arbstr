---
phase: 21-vault-billing-wiring
reviewed: 2026-04-09T00:00:00Z
depth: standard
files_reviewed: 5
files_reviewed_list:
  - src/proxy/handlers.rs
  - src/proxy/vault.rs
  - src/router/selector.rs
  - tests/common/mod.rs
  - tests/vault_billing.rs
findings:
  critical: 0
  warning: 3
  info: 3
  total: 6
status: issues_found
---

# Phase 21: Code Review Report

**Reviewed:** 2026-04-09
**Depth:** standard
**Files Reviewed:** 5
**Status:** issues_found

## Summary

This review covers the vault billing wiring: reserve/settle/release flow in the proxy handler, the vault client, provider selection, shared test helpers, and vault integration tests.

The overall architecture is sound. The reserve-before-route ordering (BILL-02), frontier-rate reservation (BILL-05), settle-on-success/release-on-failure split (BILL-03/04), and pending settlement persistence are all correctly implemented. Three warnings and three info-level items were found.

The most impactful issues are a silent settle-for-zero on NaN cost (WR-01) and vault 429 responses not being retried in the settle/release path (WR-02). Both are financial correctness risks.

## Warnings

### WR-01: NaN f64-to-u64 cast silently settles for 0 msats

**File:** `src/proxy/handlers.rs:974` and `src/proxy/handlers.rs:1378`

**Issue:** `(c * 1000.0) as u64` is used to convert `cost_sats: f64` to millisatoshis. In Rust, casting a NaN or negative f64 to u64 saturates to 0 (defined behavior since Rust 1.45). If `cost_sats` is NaN — possible from `actual_cost_sats` when token counts overflow f64 precision at extreme values — the vault settle call is made with `actual_msats = 0`, silently underpaying the provider and crediting the full reservation as a refund. The same pattern appears at line 1378 for the streaming path.

**Fix:**
```rust
// Replace:
let actual_msats = outcome.cost_sats.map(|c| (c * 1000.0) as u64).unwrap_or(0);

// With:
let actual_msats = outcome.cost_sats
    .filter(|c| c.is_finite() && *c >= 0.0)
    .map(|c| (c * 1000.0).round() as u64)
    .unwrap_or(0);
```

Apply the same fix at line 1378 in `handle_streaming_response`. Using `.round()` also avoids truncation bias (e.g. 1.9999 ms → 1 instead of 2).

---

### WR-02: Vault 429 (RateLimited) not retried in settle/release path

**File:** `src/proxy/vault.rs:280-282`

**Issue:** `call_with_retry` (used by both `settle` and `release`) only retries on 5xx responses and network errors. A 429 response from the vault falls into the `_` arm and is returned immediately as `VaultError::RateLimited` without retry. Since `settle` and `release` are the critical post-inference billing calls, a transient rate limit causes the settlement to be written to `pending_settlements` rather than resolved immediately. Under sustained vault load this compounds: each failed settle adds a pending record, and the reconciliation loop will also hit the same 429 on replay, growing the pending queue.

Note: `reserve` does not use `call_with_retry` (it has its own send logic), so reserve correctly fails fast on 429. The issue is specific to settle/release.

**Fix:**
```rust
// In call_with_retry, add 429 to the retryable set:
match status {
    200..=299 => { /* success */ }
    429 | 500..=599 => {
        // Retryable: rate limited or server error
        let msg = response.text().await.unwrap_or_default();
        last_err = if status == 429 {
            VaultError::RateLimited
        } else {
            VaultError::Unavailable(format!("HTTP {}: {}", status, msg))
        };
        tracing::warn!(attempt = attempt + 1, url, status, "Vault call failed, retrying");
        // For 429 specifically, respect Retry-After if present (optional enhancement)
    }
    _ => {
        let msg = response.text().await.unwrap_or_default();
        return Err(VaultError::Other(format!("HTTP {}: {}", status, msg)));
    }
}
```

---

### WR-03: `spawn_vault_settle` serializes metadata with `unwrap_or_default` — silent data loss on failure

**File:** `src/proxy/handlers.rs:447`

**Issue:** When the vault settle call fails and a `PendingSettlement` is written to SQLite, the metadata field is populated via `serde_json::to_string(&metadata).unwrap_or_default()`. If serialization fails (unlikely for `SettleMetadata` but not impossible if custom serialization is added later), the pending settlement is written with an empty metadata string. On reconciliation replay, `serde_json::from_str::<SettleMetadata>("")` fails, the record increments its attempts counter forever, and the settlement is never replayed or cleaned up. There is no alerting and no dead-letter handling.

The same pattern exists at line 1417 in `handle_streaming_response`.

**Fix:**
```rust
match serde_json::to_string(&metadata) {
    Ok(meta_str) => {
        let pending = super::vault::PendingSettlement {
            settlement_type: "settle".to_string(),
            reservation_id: reservation_id.clone(),
            amount_msats: Some(actual_msats),
            metadata: meta_str,
        };
        // ...insert pending...
    }
    Err(e) => {
        tracing::error!(
            reservation_id = %reservation_id,
            error = %e,
            "CRITICAL: Cannot serialize settle metadata — settlement permanently lost"
        );
    }
}
```

## Info

### IN-01: `cancel_wait` clones the receiver it already holds a reference to

**File:** `src/proxy/vault.rs:481-489`

**Issue:** `cancel_wait` takes `cancel: &tokio::sync::watch::Receiver<bool>` then immediately calls `cancel.clone()`. The clone is unnecessary — `Receiver<bool>` implements `changed()` and `borrow_and_update()` on `&mut self`, so the function could take `cancel: &mut tokio::sync::watch::Receiver<bool>` directly and avoid the clone allocation. Minor wasted work on every reconciliation loop tick.

**Fix:** Change the signature to `async fn cancel_wait(cancel: &mut tokio::sync::watch::Receiver<bool>)` and remove the `let mut cancel = cancel.clone();` line inside the function body.

---

### IN-02: Redundant `* 1000 / 1000` in `estimate_reserve_msats`

**File:** `src/proxy/vault.rs:319-322`

**Issue:** The cost formula is `(tokens * rate * 1000) / 1000`, which simplifies to `tokens * rate`. The `* 1000 / 1000` is a no-op. The comment says "Convert to msats" but the conversion is conceptually `tokens * rate_sats_per_1k / 1000 tokens_per_k * 1000 msats_per_sat`, which does equal `tokens * rate` — the formula is numerically correct, but the code is misleading and introduces potential integer overflow for large token counts before the division.

**Fix:**
```rust
// Rates are in sats per 1000 tokens; 1 sat = 1000 msats.
// cost_msats = tokens * rate_sats_per_1k / 1000 * 1000 = tokens * rate_sats_per_1k
let input_cost_msats = estimated_input_tokens as u64 * input_rate;
let output_cost_msats = estimated_output_tokens as u64 * output_rate;
let base_fee_msats = base_fee * 1000;
```

Note: existing unit tests cover the correct expected values; they will continue to pass after this simplification.

---

### IN-03: Test `test_full_reserve_route_settle_path` relies on `tokio::time::sleep` for settle ordering

**File:** `tests/vault_billing.rs:409-439`

**Issue:** The test waits 200 ms (`tokio::time::sleep(Duration::from_millis(200))`) for the fire-and-forget `spawn_vault_settle` task to complete before asserting that `settle` was called. This is a timing-based assertion that will pass on fast CI but can flake on slow machines or under resource contention. The same pattern appears in `test_vault_release_on_provider_failure` at line 469.

A more robust approach would be to expose a way for tests to await the spawned task's completion, or to use a `tokio::sync::Notify` or channel to signal when settle/release has been called by the mock vault, rather than sleeping.

This is an existing pattern in the codebase (consistent with other test files), so no immediate action is required — but it is worth tracking as a known test reliability risk.

---

_Reviewed: 2026-04-09_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
