---
phase: 18-tier-aware-routing
reviewed: 2026-04-08T00:00:00Z
depth: standard
files_reviewed: 4
files_reviewed_list:
  - src/router/complexity.rs
  - src/router/selector.rs
  - src/router/mod.rs
  - src/proxy/handlers.rs
findings:
  critical: 1
  warning: 3
  info: 2
  total: 6
status: issues_found
---

# Phase 18: Code Review Report

**Reviewed:** 2026-04-08
**Depth:** standard
**Files Reviewed:** 4
**Status:** issues_found

## Summary

This phase introduces tier-aware routing via a heuristic complexity scorer (`complexity.rs`), tier filtering in the router (`selector.rs`), and exports through `mod.rs`. The scorer itself is well-structured with sensible defaults and good test coverage. The tier filtering in `selector.rs` is correctly implemented.

The most significant issue is a **critical logic gap**: the complexity scorer and tier-to-max-tier conversion are fully built but are never called from the request handler. Every request is routed with `max_tier: None`, making the entire tier-aware routing feature a no-op in production. Additionally, there are two warning-level correctness issues in the complexity scorer and one in the handler.

---

## Critical Issues

### CR-01: Complexity-based tier routing is never invoked — feature is a no-op

**File:** `src/proxy/handlers.rs:248`

**Issue:** `resolve_candidates` always passes `None` for `max_tier` to `select_candidates`. The `score_complexity` and `score_to_max_tier` functions exported from `src/router/mod.rs` are never called anywhere in the proxy layer. `AppState` holds `state.config` (which contains `routing: RoutingConfig` with the thresholds and weights), so all the inputs required to drive tier routing exist — they are simply never connected.

As written, all requests route to the cheapest provider regardless of tier, which means providers tagged `Tier::Local` will be selected for complex requests even when `Tier::Frontier` providers were configured for that purpose.

**Fix:** Compute the complexity score from the full message list before routing and pass the resulting `max_tier` to `select_candidates`:

```rust
// In resolve_candidates(), after building `ctx` and before calling select_candidates:
let max_tier = {
    let weights = &state.config.routing.complexity_weights;
    let score = crate::router::score_complexity(&request_messages, weights);
    let low = state.config.routing.complexity_threshold_low;
    let high = state.config.routing.complexity_threshold_high;
    Some(crate::router::score_to_max_tier(score, low, high))
};

// Then pass max_tier instead of None:
state.router.select_candidates(&ctx.model, ctx.policy_name.as_deref(), user_prompt, max_tier)
```

Note that `resolve_candidates` currently only receives `user_prompt: Option<&str>` (the last user message), but `score_complexity` needs the full `&[Message]` slice. The function signature will need to accept the full message list, or the scoring can be done in `chat_completions` before calling `resolve_candidates` and the resulting `max_tier` threaded through.

---

## Warnings

### WR-01: Multimodal `Parts` content — only first text part is scored

**File:** `src/router/complexity.rs:81`

**Issue:** `score_complexity` builds `text` by calling `m.content.as_str()` on each message, which for `MessageContent::Parts` returns only the text from the **first** text-type part. All subsequent text parts and all non-text parts are silently dropped. This means a multimodal message with multiple text blocks (e.g., a system prompt split across parts, or interleaved text+image parts) will be under-scored, biasing routing toward cheaper tiers for complex requests.

```rust
// Current — drops all but the first text part per message:
let text: String = messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
```

**Fix:** Concatenate all text parts from each message:

```rust
fn message_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type")?.as_str()? == "text" {
                    p.get("text")?.as_str().map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

let text: String = messages.iter().map(|m| message_text(&m.content)).collect::<Vec<_>>().join("\n");
```

### WR-02: `score_to_max_tier` boundary: `score == high` maps to `Standard`, not `Frontier`

**File:** `src/router/complexity.rs:56-64`

**Issue:** The tier boundary condition `score > high` means a score exactly equal to `high` (e.g., 0.7) resolves to `Tier::Standard`, not `Tier::Frontier`. The comment says "score > high -> Frontier" which is accurate, but the intent for the boundary value is ambiguous. Given default thresholds of `low=0.4, high=0.7`, a score of exactly 0.7 gets `Standard` tier. This is a very narrow edge case but could silently under-route requests that score precisely at the upper threshold.

More importantly, the tests at line 368 assert `score_to_max_tier(0.7, 0.4, 0.7) == Tier::Standard`, locking in this behavior, but the `RoutingConfig` docs do not document whether the upper boundary is exclusive. If the intent is `score >= high -> Frontier`, the condition and test should both be updated.

**Fix (if intended to be inclusive on the upper bound):**

```rust
pub fn score_to_max_tier(score: f64, low: f64, high: f64) -> Tier {
    if score < low {
        Tier::Local
    } else if score >= high {   // inclusive upper bound
        Tier::Frontier
    } else {
        Tier::Standard
    }
}
```

If exclusive is the correct intent, add a doc comment explicitly stating the boundary is exclusive to prevent future confusion.

### WR-03: Tier filter error on no-match uses `NoPolicyMatch` — wrong error variant

**File:** `src/router/selector.rs:116-119`

**Issue:** When tier filtering eliminates all candidates, the code returns `Err(Error::NoPolicyMatch)`. This error is semantically incorrect: the request was not rejected by a policy rule, it was rejected because no provider in the required tier was available. At the HTTP boundary, both `NoPolicyMatch` and `NoProviders` map to `400 Bad Request` (via `routing_error_status`), so the user receives a 400 without a clear message explaining that the failure was due to tier constraints rather than a policy or model mismatch.

```rust
// Line 116-119 in selector.rs:
if candidates.is_empty() {
    return Err(Error::NoPolicyMatch);  // misleading error type
}
```

**Fix:** Either reuse `Error::NoProviders` with the model name (which clearly communicates "no providers available for this request") or add a dedicated `Error::NoTierMatch { tier: Tier }` variant:

```rust
if candidates.is_empty() {
    return Err(Error::NoProviders { model: model.to_string() });
}
```

---

## Info

### IN-01: `signal_multi_file` conflates file path matches with standalone extension matches

**File:** `src/router/complexity.rs:129-139`

**Issue:** `signal_multi_file` inserts both full file paths (e.g., `src/main.rs`) and standalone extensions (e.g., `.rs`) into the same `HashSet`. A single file reference like `src/main.rs` produces two distinct entries in the set: `"src/main.rs"` from `FILE_PATH_RE` and `".rs"` from `FILE_EXT_RE`. This inflates the count by up to 2x for single-file references, making the signal saturate at 3 entries (score = 1.0) with as few as two file paths that share an extension.

This is not a correctness bug (the signal is bounded to `[0.0, 1.0]`), but it means the signal saturates faster than the doc comment implies ("3 unique file path references").

**Fix:** Either use two separate counters with separate normalization denominators, or deduplicate by only inserting canonical path matches and treating extensions as a bonus signal.

### IN-02: `unwrap()` on `serde_json::to_vec` in vault backpressure response body

**File:** `src/proxy/handlers.rs:466`

**Issue:** The vault backpressure error path at line 466 calls `.unwrap()` on `serde_json::to_vec(...)`. While serializing a static JSON literal is practically infallible, using `.unwrap()` in async handler code is inconsistent with the rest of the file's error handling pattern (which uses `.map_err(...)` or `.unwrap_or_default()`). A panic here would crash the handler task without returning an HTTP response. The same pattern appears at line 496 (vault 401 response).

**Fix:** Replace with a fallback:

```rust
.body(Body::from(
    serde_json::to_vec(&serde_json::json!({ ... }))
        .unwrap_or_else(|_| b"{}".to_vec())
))
```

---

_Reviewed: 2026-04-08_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
