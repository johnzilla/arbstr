---
phase: 17-complexity-scorer
reviewed: 2026-04-08T00:00:00Z
depth: standard
files_reviewed: 4
files_reviewed_list:
  - src/router/complexity.rs
  - src/router/mod.rs
  - src/config.rs
  - Cargo.toml
findings:
  critical: 0
  warning: 3
  info: 3
  total: 6
status: issues_found
---

# Phase 17: Code Review Report

**Reviewed:** 2026-04-08
**Depth:** standard
**Files Reviewed:** 4
**Status:** issues_found

## Summary

This phase introduces the heuristic complexity scorer (`src/router/complexity.rs`) and supporting config types (`ComplexityWeightsConfig`, `RoutingConfig`) in `src/config.rs`. The scorer itself is well-structured: five weighted signals, sigmoid functions for smooth gradients, a safe default-to-frontier fallback, and a solid test suite. The logic is correct for the happy path.

Three warnings stand out: the scorer is not yet wired into any request path (dead export), the `MultiModal` path in `as_str()` silently drops all text parts except the first, and there is no validation that `complexity_threshold_low < complexity_threshold_high`, which will produce silent mis-routing if misconfigured. Three info items cover a loose test assertion, a double-allocation on every score call, and a missing negative-weight guard.

---

## Warnings

### WR-01: `score_complexity` is exported but never called — scorer is not integrated

**File:** `src/router/mod.rs:11`
**Issue:** `pub use complexity::score_complexity` is exported, but grepping the entire `src/` tree finds zero call sites outside of the module's own tests. `RoutingConfig` (including `complexity_threshold_low` / `complexity_threshold_high`) is also parsed but never read at request time. The feature is therefore fully dead in production — requests are not routed by complexity regardless of config.

This is noted as intentional in the `RoutingConfig` doc comment ("routing logic is implemented in Phase 18"), but it means the Phase 17 deliverable cannot be validated end-to-end and any user who sets `[routing]` in their config will see no effect without a visible warning.

**Fix:** Either add a `tracing::warn!` at startup when `routing` config is present (so operators know it is not yet active), or ensure Phase 18 is gated before this ships. The warning approach:
```rust
// In server startup, after config load:
if config.routing.complexity_threshold_low != 0.4
    || config.routing.complexity_threshold_high != 0.7
{
    tracing::warn!(
        "[routing] complexity thresholds configured but scorer not yet wired — Phase 18 required"
    );
}
```

---

### WR-02: Multimodal `as_str()` silently discards all text parts after the first

**File:** `src/proxy/types.rs:72-82` (consumed by `complexity.rs:66`)
**Issue:** `MessageContent::as_str()` uses `find_map`, which returns only the **first** text part and ignores any subsequent text parts in a multimodal message. For a message like:
```json
[{"type":"text","text":"intro"}, {"type":"image_url","..."}, {"type":"text","text":"follow-up question with keywords"}]
```
the second text block is silently dropped. The scorer will undercount tokens, miss keywords, and potentially under-score a complex multi-part prompt.

This is not a crash — the fallback is `""` — but it causes incorrect scoring for a valid input class.

**Fix:** Collect and join all text parts instead of stopping at the first:
```rust
MessageContent::Parts(parts) => {
    // Collect all text parts — multimodal messages may have several.
    // Leaks a temporary String; acceptable given scoring is per-request.
    let joined: String = parts
        .iter()
        .filter_map(|p| {
            if p.get("type")?.as_str()? == "text" {
                p.get("text")?.as_str()
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    // NOTE: as_str() returns &str — returning a joined String requires
    // changing the return type to Cow<str> or storing in a field.
    // Alternatively, score_complexity can call a separate fn that returns String.
    todo!("return Cow::Owned(joined) after changing signature")
}
```
The cleanest fix is to change `as_str() -> &str` to `to_text() -> String` (or `Cow<'_, str>`) so ownership of the joined string is possible without a lifetime issue. The scorer at `complexity.rs:66` already allocates a `String` via `.collect()`, so accepting `String` here costs nothing extra.

---

### WR-03: No validation that `complexity_threshold_low < complexity_threshold_high`

**File:** `src/config.rs:404-420`
**Issue:** `Config::validate()` does not check that `complexity_threshold_low < complexity_threshold_high`. If a user sets them inverted (e.g., `low = 0.8`, `high = 0.3`) the router in Phase 18 will see every request fall into the "standard" band — no request would ever route to local or frontier. This will be a silent mis-routing that is very hard to debug.

**Fix:** Add to the `validate` method:
```rust
let r = &self.routing;
if r.complexity_threshold_low >= r.complexity_threshold_high {
    return Err(ConfigError::Validation(format!(
        "routing.complexity_threshold_low ({}) must be less than \
         complexity_threshold_high ({})",
        r.complexity_threshold_low, r.complexity_threshold_high
    )));
}
if !(0.0..=1.0).contains(&r.complexity_threshold_low)
    || !(0.0..=1.0).contains(&r.complexity_threshold_high)
{
    return Err(ConfigError::Validation(
        "routing thresholds must be in [0.0, 1.0]".to_string(),
    ));
}
```

---

## Info

### IN-01: `test_zero_weight_eliminates_signal` has a weak assertion that can vacuously pass

**File:** `src/router/complexity.rs:282-285`
**Issue:** The assertion is:
```rust
assert!(
    (score_normal - score_zero).abs() > 0.01 || score_normal < 0.3,
    ...
);
```
The `|| score_normal < 0.3` escape hatch means the test passes even when zeroing `code_blocks` has no measurable effect, as long as the normal score happens to be below 0.3. The test is designed to verify that the weight actually matters, but the disjunction undermines that intent — future weight-tuning could flip the branch silently.

**Fix:** Remove the escape hatch and assert the difference directly:
```rust
assert!(
    (score_normal - score_zero).abs() > 0.01,
    "Zeroing code_blocks weight should change the score; normal={score_normal}, zero={score_zero}"
);
```
If the test message does not produce a large enough code-block signal on its own, strengthen the message (more code blocks, minimal other signals).

---

### IN-02: Per-call `String` allocation for text concatenation can be avoided with a borrowed approach

**File:** `src/router/complexity.rs:66`
**Issue:**
```rust
let text: String = messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
```
This allocates an intermediate `Vec<&str>` before joining. `itertools::join` or a fold into a single `String` avoids the intermediate vector. Minor, but worth noting since this runs on every proxied request.

**Fix:**
```rust
let mut text = String::new();
for (i, m) in messages.iter().enumerate() {
    if i > 0 { text.push('\n'); }
    text.push_str(m.content.as_str());
}
```
Or use `itertools::join` if the crate is already in the dependency tree (it is not currently; stick with the manual fold).

---

### IN-03: No guard against negative weight values in `ComplexityWeightsConfig`

**File:** `src/config.rs:215-229`
**Issue:** Weights are `f64` with no lower-bound validation. A negative weight (e.g., `context_length = -1.0`) inverts that signal's contribution, meaning longer context would *lower* the complexity score. This is almost certainly a misconfiguration rather than intended behavior. Because `weight_total` sums all weights, a sufficiently negative weight can make `weight_total` zero or negative, skipping the guard at `complexity.rs:92` and producing NaN/infinity before the clamp (the clamp handles infinity but not NaN — `NaN.clamp(0.0, 1.0)` returns NaN in Rust).

**Fix:** Add a validation step in `Config::validate()` (or in `ComplexityWeightsConfig` itself):
```rust
for (name, w) in [
    ("context_length", cw.context_length),
    ("code_blocks", cw.code_blocks),
    ("multi_file", cw.multi_file),
    ("reasoning_keywords", cw.reasoning_keywords),
    ("conversation_depth", cw.conversation_depth),
] {
    if w < 0.0 {
        return Err(ConfigError::Validation(format!(
            "routing.complexity_weights.{name} must be >= 0.0, got {w}"
        )));
    }
}
```
Alternatively, make the clamp NaN-safe: `(weighted_sum / weight_total).clamp(0.0, 1.0)` does not protect against NaN inputs since `NaN.clamp(a, b) == NaN` in Rust. Add an explicit NaN check: `.max(0.0).min(1.0)` or `if score.is_nan() { 1.0 } else { score }`.

---

_Reviewed: 2026-04-08_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
