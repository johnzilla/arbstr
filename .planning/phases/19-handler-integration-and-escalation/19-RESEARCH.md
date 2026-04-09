# Phase 19: Handler Integration and Escalation - Research

**Researched:** 2026-04-08
**Domain:** Rust/axum handler modification, tier escalation logic, circuit breaker interaction
**Confidence:** HIGH

## Summary

Phase 19 wires the complexity scorer (built in Phase 17) and tier-aware routing (built in Phase 18) into the live request handler. The primary modification target is `resolve_candidates()` in `src/proxy/handlers.rs`, which currently passes `None` as `max_tier` to `select_candidates`. This phase adds three capabilities: (1) scoring requests and passing the resulting tier to `select_candidates`, (2) parsing `X-Arbstr-Complexity` header as an override, and (3) automatic tier escalation when circuit breakers block all providers at the scored tier.

The highest risk in this phase is the escalation logic. The existing retry loop (`retry_with_fallback`) operates on a flat candidate list and handles transient provider failures. Escalation adds a second dimension -- provider *availability* at a tier level. These two mechanisms must not interfere. The research and CONTEXT.md decisions are aligned: escalation happens inside `resolve_candidates` before the retry loop sees candidates, expanding the candidate pool one-way (never de-escalating). This keeps the retry loop unchanged.

A secondary concern is vault billing: the current code reserves funds based on the cheapest candidate's rates. After escalation, candidates may include more expensive frontier providers. The reserve estimate should use the most expensive candidate in the final (potentially escalated) list, or at minimum the frontier-tier rates when vault is configured.

**Primary recommendation:** Modify `resolve_candidates` to accept `&ChatCompletionRequest` and `&RoutingConfig`, add header parsing before scoring, implement a simple for-loop over `[initial_tier, next_tier, Frontier]` with early break on finding candidates, and return the expanded+filtered candidate list to the existing retry loop unchanged.

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Score the request inside `resolve_candidates()`. This function already calls `select_candidates` and is shared by both streaming and non-streaming paths.
- **D-02:** `resolve_candidates` needs access to `&[Message]` from the request body (for scoring) and `&RoutingConfig` from `AppState.config` (for weights and thresholds).
- **D-03:** Call `score_complexity(&request.messages, &config.routing.complexity_weights)` to get the score, then `score_to_max_tier(score, config.routing.complexity_threshold_low, config.routing.complexity_threshold_high)` to get `max_tier`.
- **D-04:** Pass the computed `Some(max_tier)` to `select_candidates` instead of `None`.
- **D-05:** Escalation happens inside `resolve_candidates`. If `select_candidates` returns `NoPolicyMatch` with a tier filter active, try the next tier up.
- **D-06:** Escalation order: `Local -> Standard -> Frontier`. Maximum 2 escalation attempts per request.
- **D-07:** One-way only -- never de-escalate. Once escalated, the expanded tier is final for the request.
- **D-08:** Reuse existing circuit breaker state -- `select_candidates` already gets filtered candidates; circuit breaker filtering happens in the handler's retry loop. Escalation expands the candidate pool before retry.
- **D-09:** Log escalation at WARN level: "Tier escalation: {from_tier} -> {to_tier} (no healthy providers at {from_tier})"
- **D-10:** Parse `X-Arbstr-Complexity` header from request. Case-insensitive value matching.
- **D-11:** Valid values: `high` -> `Tier::Frontier`, `medium` -> `Tier::Standard`, `low` -> `Tier::Local`.
- **D-12:** Invalid or missing header -> fall through to scorer (no error, no warning).
- **D-13:** Header override skips the scorer entirely -- the header value IS the max_tier, no scoring needed.
- **D-14:** Header is checked before scoring so we skip the scorer computation when override is present.

### Claude's Discretion
- Whether `resolve_candidates` signature needs to change or if it accesses messages/config through existing params
- Exact error matching for NoPolicyMatch in escalation loop
- Whether to add a dedicated `NoTierMatch` error variant or reuse `NoPolicyMatch`

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope.

</user_constraints>

<phase_requirements>

## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SCORE-03 | `X-Arbstr-Complexity: high\|low` header overrides the scorer | D-10 through D-14 define header parsing; existing `ARBSTR_POLICY_HEADER` pattern in handlers.rs provides template |
| ROUTE-04 | When scored tier has no healthy providers (circuit broken), router escalates to next tier automatically | D-05 through D-08 define escalation loop; `select_candidates` returns `Err(NoPolicyMatch)` when tier filter yields empty; escalation retries with next tier |
| ROUTE-05 | Escalation is one-way per request (local -> standard -> frontier, never de-escalates) | D-06 and D-07 enforce one-way; `Tier::escalate()` helper returns `Some(next)` or `None` at Frontier |

</phase_requirements>

## Architecture Patterns

### Current `resolve_candidates` Function (lines 240-312)

The function currently:
1. Calls `state.router.select_candidates(&ctx.model, ctx.policy_name.as_deref(), user_prompt, None)` -- always passes `None` for `max_tier`
2. On routing error: logs, builds error response, returns `Err(response)`
3. Iterates candidates through circuit breaker permits (`acquire_permit`)
4. If all filtered out by circuit breakers: returns 503 `CircuitOpen` error
5. Returns `ResolvedCandidates { candidates, probe_provider }`

**Signature:** `async fn resolve_candidates(state: &AppState, ctx: &RequestContext, user_prompt: Option<&str>) -> Result<ResolvedCandidates, Response>`

**Key insight:** The function already has access to `state.config` (via `AppState`) and `state.router`. It does NOT currently have access to `&[Message]` from the request body. The signature must change. [VERIFIED: codebase read]

### Pattern 1: Signature Change for `resolve_candidates`

**What:** Add `messages: &[Message]` and optionally `headers: &HeaderMap` to the function parameters. Access `state.config.routing` for thresholds/weights.

**Current call site** (line 438):
```rust
let resolved = match resolve_candidates(&state, &ctx, user_prompt).await {
    Ok(r) => r,
    Err(response) => return Ok(response),
};
```

**New call site:**
```rust
let resolved = match resolve_candidates(&state, &ctx, user_prompt, &request.messages, &headers).await {
    Ok(r) => r,
    Err(response) => return Ok(response),
};
```

**Why pass headers:** The `X-Arbstr-Complexity` header must be parsed before scoring. While `ctx` carries `policy_name` (extracted from headers earlier), the complexity header is not yet extracted. Passing the full `HeaderMap` follows the existing pattern where `policy_name` is extracted from headers in the handler and stored in `ctx`. An alternative is to extract the complexity override in `chat_completions` and pass it as `Option<Tier>` -- this is cleaner. [VERIFIED: codebase read]

**Recommended approach:** Extract the complexity header override in `chat_completions` before calling `resolve_candidates`, similar to how `policy_name` is extracted. Pass `complexity_override: Option<Tier>` to `resolve_candidates`. This keeps the function signature clean and follows the existing pattern.

### Pattern 2: Header Parsing (X-Arbstr-Complexity)

**What:** Parse `X-Arbstr-Complexity` header with case-insensitive value matching.

**Existing pattern** (lines 415-418):
```rust
let policy_name = headers
    .get(ARBSTR_POLICY_HEADER)
    .and_then(|v| v.to_str().ok())
    .map(|s| s.to_string());
```

**New pattern:**
```rust
pub const ARBSTR_COMPLEXITY_HEADER: &str = "x-arbstr-complexity";

let complexity_override: Option<Tier> = headers
    .get(ARBSTR_COMPLEXITY_HEADER)
    .and_then(|v| v.to_str().ok())
    .and_then(|s| match s.to_lowercase().as_str() {
        "high" => Some(Tier::Frontier),
        "medium" => Some(Tier::Standard),
        "low" => Some(Tier::Local),
        _ => None, // D-12: invalid -> fall through to scorer
    });
```

[VERIFIED: follows existing header parsing pattern in handlers.rs]

### Pattern 3: Tier Escalation Loop

**What:** When `select_candidates` returns `NoPolicyMatch` with tier filtering active, try the next tier.

**Implementation approach:**
```rust
// Inside resolve_candidates:
let routing = &state.config.routing;

// Determine initial max_tier
let (score, max_tier) = if let Some(override_tier) = complexity_override {
    (None, override_tier) // header override, no scoring
} else {
    let s = score_complexity(messages, &routing.complexity_weights);
    let t = score_to_max_tier(s, routing.complexity_threshold_low, routing.complexity_threshold_high);
    (Some(s), t)
};

// Escalation loop
let mut current_tier = max_tier;
let candidates = loop {
    match state.router.select_candidates(&ctx.model, ctx.policy_name.as_deref(), user_prompt, Some(current_tier)) {
        Ok(c) => break c,
        Err(Error::NoPolicyMatch) => {
            if let Some(next) = current_tier.escalate() {
                tracing::warn!(
                    from_tier = %current_tier,
                    to_tier = %next,
                    "Tier escalation: {} -> {} (no healthy providers at {})",
                    current_tier, next, current_tier
                );
                current_tier = next;
            } else {
                // Already at Frontier, no further escalation
                // Fall through to existing error handling
                break return Err(/* error response */);
            }
        }
        Err(e) => {
            // NoProviders or other non-tier error -- no escalation
            break return Err(/* error response */);
        }
    }
};
```

**Critical detail:** The escalation loop triggers on `NoPolicyMatch` from `select_candidates`. Looking at selector.rs line 116-118, when `max_tier` is set and `candidates.retain(|p| p.tier <= max_tier)` empties the list, it returns `Err(Error::NoPolicyMatch)`. This is the correct error to match. However, `NoPolicyMatch` is also returned when policy constraints (cost limits, allowed models) empty the list (line 199). The escalation loop must distinguish between "no providers at this tier" and "no providers match policy constraints at any tier." [VERIFIED: codebase read of selector.rs]

**Recommendation on error variant (Claude's discretion):** Add a `NoTierMatch` error variant. This makes the escalation loop unambiguous -- only escalate on `NoTierMatch`, not on `NoPolicyMatch` from policy constraints. Without this, a cost-constrained policy that eliminates all providers would trigger unnecessary escalation attempts. The variant can map to the same HTTP status as `NoPolicyMatch`.

### Pattern 4: `Tier::escalate()` Helper

**What:** Add a method to `Tier` enum for one-way escalation.

```rust
impl Tier {
    /// Return the next tier up, or None if already at Frontier.
    pub fn escalate(&self) -> Option<Tier> {
        match self {
            Tier::Local => Some(Tier::Standard),
            Tier::Standard => Some(Tier::Frontier),
            Tier::Frontier => None,
        }
    }
}
```

No `impl Tier` block exists currently. This would be the first. [VERIFIED: grep found no `impl Tier` in src/]

### Anti-Patterns to Avoid

- **Re-scoring during escalation:** The complexity score is computed once per request. Escalation changes the tier filter, NOT the score. D-08 confirms this.
- **Escalation inside the retry loop:** Escalation is a candidate-pool concern, not a transient-failure concern. It must complete BEFORE the retry loop sees candidates. D-08 confirms this.
- **Two-way escalation:** A request that escalated from Local to Standard must never go back to Local. The loop variable `current_tier` only moves upward. D-07 confirms this.
- **Escalation on `NoProviders`:** If the model itself has no providers (regardless of tier), escalation will not help. Only escalate on tier-filtering failures.

## Vault Billing Interaction

**Current behavior:** Reserve amount is estimated using `cheapest` candidate's rates (line 507-516 of handlers.rs). This happens AFTER `resolve_candidates` returns.

**Impact of escalation:** If `resolve_candidates` escalates from Local to Standard, the candidate list now includes Standard-tier providers which are more expensive. The cheapest candidate in the escalated list may have higher rates than the original Local-tier cheapest. The current vault reservation logic (`&resolved.candidates[0]`) will naturally use the cheapest available candidate from the escalated pool. This is correct -- the reservation reflects the actual cheapest option available. [VERIFIED: codebase read]

**STATE.md concern about frontier pricing:** STATE.md notes "vault reservation under tier escalation: when local request might escalate to frontier, vault reservation must use frontier-tier pricing." However, since escalation happens INSIDE `resolve_candidates` (before vault reservation), the reservation already sees the escalated candidate list. The cheapest candidate in that list is the one used for reservation. This handles the concern naturally -- no special frontier-pricing logic needed. The reservation will be based on the cheapest candidate that actually survived escalation. [VERIFIED: code flow analysis]

## Common Pitfalls

### Pitfall 1: Escalation on Policy Mismatch vs Tier Mismatch
**What goes wrong:** `NoPolicyMatch` is returned both when tier filtering empties candidates AND when policy cost constraints eliminate providers. Escalating on policy mismatch is wasteful -- higher tiers are more expensive and even less likely to pass cost constraints.
**Why it happens:** `select_candidates` reuses `NoPolicyMatch` for both cases (lines 117 and 199 of selector.rs).
**How to avoid:** Add `NoTierMatch` error variant in `error.rs` and use it at line 117 of selector.rs (tier filter empty). Match only `NoTierMatch` in escalation loop. Let `NoPolicyMatch` pass through as a non-escalatable error.
**Warning signs:** Escalation logging fires even when the real issue is cost constraints.

### Pitfall 2: Circuit Breaker Filtering After Escalation
**What goes wrong:** `resolve_candidates` escalates to Standard because Local had no providers. But the circuit breaker filtering (lines 269-287) then removes the Standard providers too. Result: empty candidate list, 503 error.
**Why it happens:** Escalation is based on provider configuration (select_candidates), but circuit breakers reflect runtime health. A tier can have configured providers that are all circuit-broken.
**How to avoid:** After escalation + circuit breaker filtering, if the filtered list is empty AND we haven't tried all tiers, escalate again. This means the escalation loop should wrap BOTH the select_candidates call AND the circuit breaker filtering.
**Warning signs:** 503 errors when providers exist at higher tiers but all providers at the escalated tier are circuit-broken.

**IMPORTANT:** This is a subtle design point. D-05 says "If `select_candidates` returns `NoPolicyMatch` with a tier filter active, try the next tier up." But circuit breakers can also empty the list AFTER select_candidates succeeds. The escalation loop should cover both cases. The recommended approach: move the entire select+circuit-breaker-filter sequence into the escalation loop, not just the select call.

### Pitfall 3: Max 2 Escalation Attempts Miscounted
**What goes wrong:** D-06 says "Maximum 2 escalation attempts per request." With 3 tiers (Local, Standard, Frontier), starting at Local means 2 escalation steps (Local->Standard, Standard->Frontier). Starting at Standard means 1 step. Starting at Frontier means 0. A counter-based limit could over- or under-restrict.
**How to avoid:** The one-way escalation via `Tier::escalate()` returning `None` at Frontier naturally limits attempts. No explicit counter needed -- the tier enum itself bounds escalation. The "maximum 2" constraint is inherent in having 3 tiers.

### Pitfall 4: Scoring Overhead on Overridden Requests
**What goes wrong:** Running the scorer when `X-Arbstr-Complexity` header is present wastes CPU on regex matching and text analysis.
**How to avoid:** D-14 explicitly requires checking the header BEFORE scoring. The code structure must be `if header { use header } else { score }`.

## Code Examples

### Complete `resolve_candidates` Rewrite Pattern

```rust
/// Custom header for complexity tier override.
pub const ARBSTR_COMPLEXITY_HEADER: &str = "x-arbstr-complexity";

async fn resolve_candidates(
    state: &AppState,
    ctx: &RequestContext,
    user_prompt: Option<&str>,
    messages: &[Message],
    complexity_override: Option<Tier>,
) -> Result<ResolvedCandidates, Response> {
    let routing = &state.config.routing;

    // Step 1: Determine max_tier (header override or scorer)
    let max_tier = if let Some(tier) = complexity_override {
        tier
    } else {
        let score = score_complexity(messages, &routing.complexity_weights);
        score_to_max_tier(score, routing.complexity_threshold_low, routing.complexity_threshold_high)
    };

    // Step 2: Select + circuit-filter with escalation
    let mut current_tier = max_tier;
    loop {
        // Try select_candidates at current tier
        let candidates = match state.router.select_candidates(
            &ctx.model, ctx.policy_name.as_deref(), user_prompt, Some(current_tier),
        ) {
            Ok(c) => c,
            Err(Error::NoPolicyMatch) | Err(Error::NoTierMatch { .. }) => {
                // No providers at this tier -- try escalating
                if let Some(next) = current_tier.escalate() {
                    tracing::warn!(/* D-09 message */);
                    current_tier = next;
                    continue;
                }
                // At Frontier, can't escalate further
                // ... return error response ...
            }
            Err(e) => {
                // Non-tier error (NoProviders, BadRequest) -- no escalation
                // ... return error response ...
            }
        };

        // Circuit breaker filtering (same as current code)
        let mut filtered = Vec::new();
        let mut probe_provider = None;
        for candidate in &candidates {
            match state.circuit_breakers.acquire_permit(&candidate.name).await {
                Ok(PermitType::Normal) => filtered.push(candidate.clone()),
                Ok(PermitType::Probe) => {
                    probe_provider = Some(candidate.name.clone());
                    filtered.insert(0, candidate.clone());
                }
                Err(_) => { /* skip circuit-open */ }
            }
        }

        if filtered.is_empty() {
            // All providers at this tier are circuit-broken -- try escalating
            if let Some(next) = current_tier.escalate() {
                tracing::warn!(/* D-09 message */);
                current_tier = next;
                continue;
            }
            // At Frontier, can't escalate -- return 503
            // ... return CircuitOpen error response ...
        }

        return Ok(ResolvedCandidates { candidates: filtered, probe_provider });
    }
}
```

### Error Variant Addition

```rust
// In src/error.rs
#[error("No providers match tier '{tier}' for model '{model}'")]
NoTierMatch { tier: Tier, model: String },

// In into_response match:
Error::NoTierMatch { .. } => (StatusCode::BAD_REQUEST, self.to_string()),
```

### Selector Change (selector.rs line 117)

```rust
// Current:
if candidates.is_empty() {
    return Err(Error::NoPolicyMatch);
}

// Changed to:
if candidates.is_empty() {
    return Err(Error::NoTierMatch {
        tier: max_tier,
        model: model.to_string(),
    });
}
```

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Tier escalation sequence | Manual tier arrays or match chains | `Tier::escalate() -> Option<Tier>` method | Single source of truth for tier ordering, compile-time exhaustive |
| Case-insensitive header matching | Custom ASCII folding | `.to_lowercase().as_str()` match | Rust's Unicode-aware lowercase handles all header value edge cases |
| Circuit breaker state queries | Direct DashMap access | Existing `acquire_permit` API | Permit model handles Probe/Normal/Open states correctly |

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `resolve_candidates` passes `None` for max_tier | Passes scored/overridden tier | This phase | All requests now tier-filtered |
| Flat candidate list (no tier awareness) | Tier-escalated candidate list | This phase | Graceful degradation on circuit breaks |
| No complexity header | `X-Arbstr-Complexity` override | This phase | Power user escape hatch |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `NoTierMatch` error variant is better than reusing `NoPolicyMatch` | Pitfall 1, Architecture Pattern 3 | LOW -- if wrong, escalation triggers on policy mismatches too (wasteful but not broken) |
| A2 | Circuit breaker filtering should be inside the escalation loop | Pitfall 2 | MEDIUM -- if excluded, requests fail with 503 when higher-tier providers are available but lower-tier providers are circuit-broken |

## Open Questions

1. **Should the escalation loop include circuit breaker filtering?**
   - What we know: D-05 says escalation triggers on `NoPolicyMatch` from `select_candidates`. Circuit breaker filtering happens separately.
   - What's unclear: If a tier has configured providers but all are circuit-broken, should that trigger escalation? The PITFALLS.md research says yes (candidate list expansion approach). D-08 says "circuit breaker filtering happens in the handler's retry loop" suggesting it's separate from escalation.
   - Recommendation: Include circuit breaker filtering in the escalation loop. A tier with all circuit-broken providers is effectively unavailable. This matches the PITFALLS.md recommendation of "candidate list expansion" and prevents unnecessary 503s.

2. **Should `resolve_candidates` return the computed score for later use (response headers, Phase 20)?**
   - What we know: Phase 20 (OBS-01, OBS-02) will need the complexity score and tier for response headers and SSE metadata.
   - What's unclear: Whether to compute the score now and thread it through, or recompute it in Phase 20.
   - Recommendation: Extend `ResolvedCandidates` to include `complexity_score: Option<f64>` and `tier: Tier` now. This avoids re-scoring in Phase 20 and makes the struct self-documenting.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test + cargo test |
| Config file | Cargo.toml (workspace root) |
| Quick run command | `cargo test --lib` |
| Full suite command | `cargo test` |

### Phase Requirements -> Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SCORE-03 | X-Arbstr-Complexity header overrides scorer | integration | `cargo test --test handler_integration` | Wave 0 |
| ROUTE-04 | Escalation when tier has no healthy providers | unit + integration | `cargo test resolve_candidates` | Wave 0 |
| ROUTE-05 | Escalation is one-way (never de-escalates) | unit | `cargo test tier_escalate` | Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --lib`
- **Per wave merge:** `cargo test`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `Tier::escalate()` unit tests in `src/config.rs` -- covers ROUTE-05
- [ ] `resolve_candidates` unit tests with mock providers at different tiers -- covers ROUTE-04
- [ ] Integration test for `X-Arbstr-Complexity` header -- covers SCORE-03
- [ ] Integration test for escalation on circuit-broken tier -- covers ROUTE-04

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | N/A (no auth changes) |
| V3 Session Management | no | N/A |
| V4 Access Control | no | N/A |
| V5 Input Validation | yes | Header value validated via match against known strings; invalid values silently ignored per D-12 |
| V6 Cryptography | no | N/A |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Header injection via X-Arbstr-Complexity | Tampering | Value matched against fixed set (high/medium/low); unknown values fall through to scorer |
| Forced escalation by setting header to `low` | Elevation of privilege | `low` routes to cheapest tier; this is cost optimization, not privilege. No security risk |

## Sources

### Primary (HIGH confidence)
- `src/proxy/handlers.rs` -- full `resolve_candidates` function, `chat_completions` handler, `RequestContext` struct, header parsing patterns
- `src/router/selector.rs` -- `select_candidates` with `max_tier: Option<Tier>` parameter, `NoPolicyMatch` error paths
- `src/router/complexity.rs` -- `score_complexity` and `score_to_max_tier` function signatures and behavior
- `src/config.rs` -- `Tier` enum (derive ordering, Display), `RoutingConfig`, `ComplexityWeightsConfig`
- `src/error.rs` -- `Error` enum variants and `into_response` mapping
- `src/proxy/server.rs` -- `AppState` struct fields

### Secondary (MEDIUM confidence)
- `.planning/research/ARCHITECTURE.md` -- escalation design, candidate list expansion approach
- `.planning/research/PITFALLS.md` -- one-way escalation constraint, vault reservation concern, integration gotchas

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - pure Rust/axum, no new dependencies
- Architecture: HIGH - all relevant code read and verified
- Pitfalls: HIGH - backed by both codebase analysis and prior research docs

**Research date:** 2026-04-08
**Valid until:** 2026-05-08 (stable Rust codebase, no external dependencies changing)
