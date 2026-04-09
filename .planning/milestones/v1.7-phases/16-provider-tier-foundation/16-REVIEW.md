---
phase: 16-provider-tier-foundation
reviewed: 2026-04-08T00:00:00Z
depth: standard
files_reviewed: 9
files_reviewed_list:
  - config.example.toml
  - src/config.rs
  - src/main.rs
  - src/proxy/handlers.rs
  - src/router/selector.rs
  - tests/circuit_integration.rs
  - tests/common/mod.rs
  - tests/cost.rs
  - tests/health.rs
findings:
  critical: 0
  warning: 2
  info: 2
  total: 4
status: issues_found
---

# Phase 16: Code Review Report

**Reviewed:** 2026-04-08
**Depth:** standard
**Files Reviewed:** 9
**Status:** issues_found

## Summary

Phase 16 introduces provider tier foundation: a `Tier` enum (`local`/`standard`/`frontier`) and `RoutingConfig` with complexity threshold and weight fields wired into `ProviderConfig`, `RawProviderConfig`, and TOML parsing. The changes are well-structured and backward-compatible (defaults preserve existing behavior). No critical issues found.

Two warnings were identified: a silent integer overflow in the latency header cast, and missing validation that ensures the low threshold is less than the high threshold. Two info items cover dead code left intentionally and a pattern that could confuse future readers.

## Warnings

### WR-01: Silent i64-to-u64 cast on latency header value

**File:** `src/proxy/handlers.rs:119`
**Issue:** `latency_ms` is typed `i64`. The line `HeaderValue::from(latency_ms as u64)` performs a wrapping bitwise reinterpretation when the value is negative. Under normal conditions latency is non-negative, but on a system with clock skew (e.g., NTP step backward) `std::time::Instant` can briefly produce a negative duration computed via subtraction from an earlier `Instant`. If that happens, the header will contain a value near `u64::MAX` (e.g., `18446744073709551615`) rather than a small number, which can confuse clients and monitoring tools. The `i64` type was chosen deliberately for SQLite compatibility; the header type should convert safely.

**Fix:**
```rust
// Replace line 119:
HeaderValue::from(latency_ms.max(0) as u64),
```

### WR-02: No validation that complexity_threshold_low < complexity_threshold_high

**File:** `src/config.rs:404` (inside `fn validate`)
**Issue:** Phase 16 adds `complexity_threshold_low` (default 0.4) and `complexity_threshold_high` (default 0.7). The `validate()` method does not check that `low < high`. A user who accidentally inverts the two values (e.g., `complexity_threshold_low = 0.8, complexity_threshold_high = 0.3`) will get a silently misconfigured scorer once the Phase 17/18 routing logic consumes these values. No request will ever route to the frontier tier — the bug will be silent and hard to diagnose.

**Fix:** Add to `fn validate`:
```rust
if self.routing.complexity_threshold_low >= self.routing.complexity_threshold_high {
    return Err(ConfigError::Validation(format!(
        "complexity_threshold_low ({}) must be less than complexity_threshold_high ({})",
        self.routing.complexity_threshold_low,
        self.routing.complexity_threshold_high,
    )));
}
```

## Info

### IN-01: `default_strategy` field is dead code with a misleading allow attribute

**File:** `src/router/selector.rs:39-41`
**Issue:** The `Router` struct holds `default_strategy: String` and suppresses the dead-code warning with `#[allow(dead_code)]` and an inline comment. This is fine as a placeholder, but the comment "Preserved for future strategy-based dispatch" is the only indication of intent. If Phase 18 does not consume this field, it may persist indefinitely and confuse new contributors into thinking it has a live code path.

**Fix:** No immediate code change needed. When Phase 18 implements strategy dispatch, remove the `#[allow(dead_code)]` attribute and connect the field. If Phase 18 will not use it, remove the field and look up the strategy from the matched `PolicyRule` directly.

### IN-02: `RoutingConfig` and `ComplexityWeightsConfig` parsed but not yet consumed by routing logic

**File:** `src/config.rs:178-245`
**Issue:** The doc comment on `RoutingConfig` states "Parsed in Phase 16 but routing logic is implemented in Phase 18" and similarly for `ComplexityWeightsConfig`. These are intentionally incomplete — noted here only so the review record captures that no consumption path exists yet. The implementation is not a bug; just worth confirming in Phase 18 that the scorer reads from `config.routing.*` rather than hard-coding defaults.

**Fix:** In Phase 18, ensure the scorer is wired to `AppState`'s `Config::routing` and add at least one integration test that verifies a non-default weight affects score output.

---

_Reviewed: 2026-04-08_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
