# Architecture Patterns

**Domain:** Complexity scoring and tier-aware routing integration for LLM proxy
**Researched:** 2026-04-08

## Recommended Architecture

### High-Level Integration Point

The complexity scorer sits **after** request parsing and **before** `resolve_candidates`. It runs in the `chat_completions` handler, immediately after extracting the user prompt and before calling `state.router.select_candidates()`. The scorer produces a `ComplexityScore` struct that feeds into tier-filtered provider selection.

```
Request -> Parse -> Score Complexity -> Select Candidates (tier-filtered) -> Circuit Breaker Filter -> Retry Loop -> Provider
                    ^^^^^^^^^^^^^^^^    ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                    NEW component       MODIFIED: Router gains tier awareness
```

### Component Boundaries

| Component | Responsibility | New/Modified | Communicates With |
|-----------|---------------|--------------|-------------------|
| `ComplexityScorer` | Score prompt complexity using heuristic signals | **NEW** `src/scorer/` | Handler (called inline) |
| `ComplexityScore` | Struct holding score value, tier, individual signal weights | **NEW** `src/scorer/mod.rs` | Handler, Router, DB logging |
| `ScorerConfig` | Configurable weights, thresholds, tier boundaries | **NEW** `src/config.rs` addition | Config parsing, Scorer |
| `ProviderConfig.tier` | New field: `local` / `standard` / `frontier` | **MODIFIED** `src/config.rs` | Router |
| `Router::select_candidates` | Filter by tier before cost-sorting | **MODIFIED** `src/router/selector.rs` | Handler |
| `resolve_candidates` | Pass tier constraint from score to router | **MODIFIED** `src/proxy/handlers.rs` | Router, Circuit Breaker |
| `RequestLog` | Add `complexity_score` and `tier` columns | **MODIFIED** `src/storage/logging.rs` | DB writer |
| Response headers | Add `x-arbstr-complexity` and `x-arbstr-tier` | **MODIFIED** `src/proxy/handlers.rs` | Client |

### Data Flow

```
1. Handler receives ChatCompletionRequest
2. Extract user_prompt (existing) + full messages array
3. ComplexityScorer.score(&request, headers) -> ComplexityScore {
       score: f32,        // 0.0 - 1.0 normalized
       tier: Tier,        // Local | Standard | Frontier
       signals: SignalBreakdown,  // for observability
   }
4. Check X-Arbstr-Tier header override (explicit tier bypass)
5. Router.select_candidates(model, policy, prompt, tier) -> Vec<SelectedProvider>
   - Filter: providers where provider.tier <= requested_tier
   - Sort: cheapest first within tier (existing logic)
6. Circuit breaker filter (existing, unchanged)
7. If all candidates filtered out by tier + circuit breaker:
   - Escalate: retry with next tier up (Standard -> Frontier)
   - This is automatic escalation, not cross-model fallback
8. Retry loop with fallback (existing, unchanged)
9. Log: complexity_score + tier to DB
10. Headers: x-arbstr-complexity, x-arbstr-tier in response
```

## New Components

### 1. Complexity Scorer (`src/scorer/mod.rs`)

Pure function, no async, no state. Takes the request and optional headers, returns a score.

```rust
/// Complexity tier for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Local,     // Simple lookups, short answers, translation
    Standard,  // Moderate reasoning, summarization, code snippets
    Frontier,  // Complex reasoning, multi-step, large context
}

/// Result of complexity scoring.
#[derive(Debug, Clone)]
pub struct ComplexityScore {
    pub score: f32,           // 0.0 - 1.0 normalized
    pub tier: Tier,           // Derived from score + thresholds
    pub signals: SignalBreakdown,
}

/// Individual signal contributions for observability.
#[derive(Debug, Clone, Serialize)]
pub struct SignalBreakdown {
    pub context_length: f32,
    pub code_blocks: f32,
    pub reasoning_keywords: f32,
    pub conversation_depth: f32,
    pub multi_file_refs: f32,
    pub system_prompt_complexity: f32,
}

/// Score a request's complexity.
pub fn score(
    request: &ChatCompletionRequest,
    config: &ScorerConfig,
    tier_override: Option<Tier>,
) -> ComplexityScore {
    // If explicit tier override via header, skip scoring
    if let Some(tier) = tier_override {
        return ComplexityScore {
            score: tier_to_default_score(tier),
            tier,
            signals: SignalBreakdown::zeroed(),
        };
    }

    // Heuristic signals (all 0.0-1.0 normalized)
    let signals = SignalBreakdown {
        context_length: score_context_length(request, config),
        code_blocks: score_code_blocks(request),
        reasoning_keywords: score_reasoning_keywords(request, config),
        conversation_depth: score_conversation_depth(request),
        multi_file_refs: score_multi_file_refs(request),
        system_prompt_complexity: score_system_prompt(request),
    };

    // Weighted sum
    let raw = signals.context_length * config.weights.context_length
        + signals.code_blocks * config.weights.code_blocks
        + signals.reasoning_keywords * config.weights.reasoning_keywords
        + signals.conversation_depth * config.weights.conversation_depth
        + signals.multi_file_refs * config.weights.multi_file_refs
        + signals.system_prompt_complexity * config.weights.system_prompt_complexity;

    let score = raw.clamp(0.0, 1.0);

    let tier = if score < config.thresholds.local_max {
        Tier::Local
    } else if score < config.thresholds.standard_max {
        Tier::Standard
    } else {
        Tier::Frontier
    };

    ComplexityScore { score, tier, signals }
}
```

**Why a pure function, not a trait/struct with state:** The scorer has no runtime state. Making it a pure function keeps it trivially testable and avoids unnecessary abstraction. If ML-based scoring is added later (explicitly out of scope), it can replace this function behind a trait at that point.

**Why `src/scorer/` not `src/policy/`:** The existing policy engine handles constraint matching (allowed models, max cost). Complexity scoring is a different concern -- it classifies the *request*, not the *routing constraints*. Keeping them separate avoids coupling scoring evolution to policy evolution.

### 2. Scorer Configuration (`src/config.rs` additions)

```rust
/// Complexity scoring configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ScorerConfig {
    #[serde(default)]
    pub weights: SignalWeights,
    #[serde(default)]
    pub thresholds: TierThresholds,
    /// Keywords that suggest complex reasoning tasks
    #[serde(default = "default_reasoning_keywords")]
    pub reasoning_keywords: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SignalWeights {
    #[serde(default = "default_context_weight")]
    pub context_length: f32,      // default: 0.25
    #[serde(default = "default_code_weight")]
    pub code_blocks: f32,         // default: 0.15
    #[serde(default = "default_reasoning_weight")]
    pub reasoning_keywords: f32,  // default: 0.25
    #[serde(default = "default_depth_weight")]
    pub conversation_depth: f32,  // default: 0.15
    #[serde(default = "default_multifile_weight")]
    pub multi_file_refs: f32,     // default: 0.10
    #[serde(default = "default_system_weight")]
    pub system_prompt_complexity: f32,  // default: 0.10
}

#[derive(Debug, Clone, Deserialize)]
pub struct TierThresholds {
    #[serde(default = "default_local_max")]
    pub local_max: f32,     // default: 0.3 (score < 0.3 = local)
    #[serde(default = "default_standard_max")]
    pub standard_max: f32,  // default: 0.7 (score < 0.7 = standard, >= 0.7 = frontier)
}
```

TOML representation:

```toml
[scoring]
reasoning_keywords = ["analyze", "explain why", "compare", "debug", "architect", "refactor"]

[scoring.weights]
context_length = 0.25
code_blocks = 0.15
reasoning_keywords = 0.25
conversation_depth = 0.15
multi_file_refs = 0.10
system_prompt_complexity = 0.10

[scoring.thresholds]
local_max = 0.3
standard_max = 0.7
```

### 3. Provider Tier Field (`src/config.rs` modification)

```rust
/// Provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    // ... existing fields ...

    /// Provider tier for complexity-based routing.
    /// Defaults to "standard" for backward compatibility.
    #[serde(default = "default_tier")]
    pub tier: Tier,
}

fn default_tier() -> Tier {
    Tier::Standard  // Backward compatible: existing configs work unchanged
}
```

TOML representation:

```toml
[[providers]]
name = "local-llama"
url = "http://localhost:11434/v1"
models = ["llama3"]
tier = "local"
input_rate = 0
output_rate = 0

[[providers]]
name = "routstr-standard"
url = "https://api.routstr.com/v1"
models = ["gpt-4o-mini"]
tier = "standard"
input_rate = 2
output_rate = 8

[[providers]]
name = "routstr-frontier"
url = "https://api.routstr.com/v1"
models = ["claude-sonnet-4", "gpt-4o"]
tier = "frontier"
input_rate = 10
output_rate = 30
base_fee = 1
```

## Modified Components

### 4. Router: Tier-Aware Selection (`src/router/selector.rs`)

The `select_candidates` method gains an optional `tier` parameter. When present, it filters providers to those at or below the requested tier before cost-sorting.

```rust
pub fn select_candidates(
    &self,
    model: &str,
    policy_name: Option<&str>,
    prompt: Option<&str>,
    max_tier: Option<Tier>,  // NEW parameter
) -> Result<Vec<SelectedProvider>> {
    // ... existing policy matching ...

    // Filter by model (existing)
    let mut candidates = /* existing filter */;

    // NEW: Filter by tier
    if let Some(tier) = max_tier {
        candidates.retain(|p| p.tier <= tier);
        if candidates.is_empty() {
            // No providers at this tier -- caller handles escalation
            return Err(Error::NoTierMatch {
                tier,
                model: model.to_string(),
            });
        }
    }

    // Apply policy constraints (existing)
    // Sort by cost (existing)
    // Deduplicate (existing)
}
```

**Key design: `Tier` implements `Ord` as `Local < Standard < Frontier`.** This makes `p.tier <= max_tier` the natural filter. A `Local` tier request only sees `Local` providers. A `Standard` request sees `Local + Standard`. A `Frontier` request sees all.

**Why filter in Router, not Handler:** The router already owns provider filtering (by model, by policy). Tier is another filter dimension in the same category. Putting it in the handler would duplicate filtering logic.

### 5. Handler: Complexity-Aware Routing with Escalation (`src/proxy/handlers.rs`)

The `chat_completions` handler changes:

```rust
// After extracting user_prompt (existing):
let tier_override = headers
    .get("x-arbstr-tier")
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.parse::<Tier>().ok());

let complexity = scorer::score(&request, &state.config.scoring, tier_override);

tracing::info!(
    complexity_score = complexity.score,
    tier = ?complexity.tier,
    "Scored request complexity"
);

// Replace existing resolve_candidates call with tier-aware version:
let resolved = match resolve_candidates_with_tier(
    &state, &ctx, user_prompt, complexity.tier
).await {
    Ok(r) => r,
    Err(response) => return Ok(response),
};
```

The `resolve_candidates` function becomes `resolve_candidates_with_tier` and handles escalation:

```rust
async fn resolve_candidates_with_tier(
    state: &AppState,
    ctx: &RequestContext,
    user_prompt: Option<&str>,
    initial_tier: Tier,
) -> Result<ResolvedCandidates, Response> {
    let tiers = escalation_sequence(initial_tier);
    // e.g., Local -> [Local, Standard, Frontier]
    //       Standard -> [Standard, Frontier]
    //       Frontier -> [Frontier]

    for tier in &tiers {
        match state.router.select_candidates(
            &ctx.model, ctx.policy_name.as_deref(), user_prompt, Some(*tier)
        ) {
            Ok(candidates) => {
                // Apply circuit breaker filter (existing logic)
                let filtered = filter_by_circuit_breaker(state, ctx, &candidates).await;
                if !filtered.candidates.is_empty() {
                    if *tier != initial_tier {
                        tracing::info!(
                            from = ?initial_tier,
                            to = ?tier,
                            "Escalated tier due to no available providers"
                        );
                    }
                    return Ok(filtered);
                }
                // All circuits open at this tier, try next
            }
            Err(_) => continue, // No providers at this tier, try next
        }
    }

    // All tiers exhausted -- return 503
    // ... existing all-circuits-open error handling ...
}
```

**Why escalation lives in the handler, not the router:** Escalation requires circuit breaker state, which the router does not (and should not) have access to. The handler already orchestrates the router + circuit breaker interaction. Escalation is a handler-level concern: "tried tier X, all circuits open, try tier X+1."

### 6. Database: New Columns (`migrations/` + `src/storage/`)

New migration:

```sql
ALTER TABLE requests ADD COLUMN complexity_score REAL;
ALTER TABLE requests ADD COLUMN tier TEXT;
```

`RequestLog` struct gains two fields:

```rust
pub complexity_score: Option<f32>,
pub tier: Option<String>,
```

INSERT query updated to include both columns. Stats endpoint gains `group_by=tier` support via the existing column-whitelist pattern.

### 7. Response Headers and SSE Metadata

Two new response headers:

```
x-arbstr-complexity: 0.72
x-arbstr-tier: frontier
```

For streaming, these go in the trailing SSE metadata event (existing pattern from v1.2):

```
data: {"arbstr":{"cost_sats":2.35,"latency_ms":1200,"complexity":0.72,"tier":"frontier"}}
```

### 8. `SelectedProvider` Gains Tier

```rust
pub struct SelectedProvider {
    // ... existing fields ...
    pub tier: Tier,  // NEW
}
```

## Patterns to Follow

### Pattern 1: Composition Over Layering
**What:** Each new concern (scoring, tier filtering) is a discrete function called from the handler, not a middleware layer.
**When:** Always for request-specific logic that needs access to parsed request body.
**Why:** axum middleware runs before body extraction. The scorer needs the parsed `ChatCompletionRequest`. Middleware would require double-parsing or shared state hacks.

### Pattern 2: Opt-In Scoring (Backward Compatible Defaults)
**What:** `tier` defaults to `Standard`, `ScorerConfig` has serde defaults for all fields, existing configs work unchanged.
**When:** Always for config additions.
**Why:** Zero-config upgrade path. Users who do not care about complexity routing get the same behavior as before (all providers treated as Standard, no tier filtering applied when no `[scoring]` section exists).

### Pattern 3: Header Override Escapes Heuristics
**What:** `X-Arbstr-Tier: frontier` bypasses the scorer entirely and forces the requested tier.
**When:** When the user knows better than the heuristic.
**Why:** Heuristics will misclassify. Power users need escape valves. Follows the existing `X-Arbstr-Policy` pattern.

### Pattern 4: Escalation as Retry Dimension
**What:** When circuit breakers block all providers at the scored tier, automatically try the next tier up.
**When:** Only during the candidate resolution phase, not during the retry loop itself.
**Why:** Tier escalation is about provider availability, not transient failures. The retry loop handles transient failures within a tier.

## Anti-Patterns to Avoid

### Anti-Pattern 1: Scorer as Middleware
**What:** Implementing complexity scoring as an axum middleware layer.
**Why bad:** Middleware runs before `Json(request)` extraction. Would require re-parsing the body or storing partial state in extensions. Adds complexity for no benefit.
**Instead:** Call scorer inline in handler after request parsing.

### Anti-Pattern 2: Tier as a Separate Routing Step
**What:** Running tier selection in a separate pass after model/policy selection.
**Why bad:** Creates two filtering passes over providers, makes the overall selection logic harder to reason about and test.
**Instead:** Add tier as another filter dimension in `select_candidates`, alongside model and policy.

### Anti-Pattern 3: Cross-Model Fallback Disguised as Escalation
**What:** When a `local` tier provider fails, silently switching to a different (frontier) model.
**Why bad:** Explicitly out of scope per PROJECT.md. Quality expectations change when the model changes. Breaks user trust.
**Instead:** Escalation only broadens the provider pool for the *same model*. If the requested model is not available at any tier, return an error. Different models at different tiers is a config choice the user makes explicitly.

### Anti-Pattern 4: Stateful Scorer
**What:** Scorer that maintains running averages, learned weights, or cached results.
**Why bad:** Adds complexity, state management, and testing burden for v1.7. ML-based scoring is explicitly out of scope.
**Instead:** Pure function. All state is in config. Learned scoring can come later behind a trait boundary.

## Scalability Considerations

| Concern | Current (single user) | Future (multi-user) |
|---------|----------------------|---------------------|
| Scorer CPU cost | Negligible (string scanning) | Still negligible -- O(message_count * keyword_count) |
| Config per user | Single global config | Per-user scoring profiles would need config overhaul |
| Tier provider pools | 3-10 providers total | Same, tiers are provider attributes not user attributes |
| DB writes | Add 2 columns to existing writes | No additional overhead |
| Header overhead | 2 new headers, minimal | Same |

## Suggested Build Order

The build order follows dependency chains and ensures each phase is independently testable and shippable.

### Phase 1: Tier Type + Provider Config
- Add `Tier` enum to config (with `Ord`, serde, Default)
- Add `tier` field to `ProviderConfig` (default: `Standard`)
- Add `tier` field to `SelectedProvider`
- Update `From<&ProviderConfig> for SelectedProvider`
- Update `config.example.toml` with tier examples
- **Tests:** Config parsing with/without tier field, Tier ordering

### Phase 2: Complexity Scorer (Pure Logic)
- Create `src/scorer/mod.rs` with `score()` function
- Add `ScorerConfig`, `SignalWeights`, `TierThresholds` to config
- Implement all 6 heuristic signal functions
- Add `[scoring]` section parsing to Config
- **Tests:** Score various prompts, verify signal contributions, tier boundary behavior, weight customization
- **Independently testable:** No integration needed, pure functions

### Phase 3: Router Tier Filtering
- Add `max_tier: Option<Tier>` parameter to `select_candidates` and `select`
- Implement tier filtering (retain where `p.tier <= max_tier`)
- Add `Error::NoTierMatch` variant
- Update all existing call sites to pass `None` (preserves behavior)
- **Tests:** Tier filtering unit tests, backward compatibility (None = no filter)

### Phase 4: Handler Integration + Escalation
- Call scorer in `chat_completions` handler
- Implement `resolve_candidates_with_tier` with escalation logic
- Parse `X-Arbstr-Tier` header override
- Add `x-arbstr-complexity` and `x-arbstr-tier` response headers
- Add complexity/tier to SSE trailing metadata
- **Tests:** Integration tests with mock providers at different tiers, escalation on circuit break

### Phase 5: Observability (DB + Stats)
- New migration: `complexity_score` and `tier` columns
- Update `RequestLog` struct and INSERT query
- Update streaming post-stream UPDATE
- Add `group_by=tier` to stats endpoint
- **Tests:** DB write/read with new columns, stats group_by=tier

### Phase 6: Cost Estimate Update
- Update `POST /v1/cost` to accept tier parameter and return tier-filtered estimate
- **Tests:** Cost estimate with tier constraint

**Phase ordering rationale:**
- Phase 1 first because `Tier` is used by everything else
- Phase 2 before Phase 3 because scorer produces the tier value that the router consumes
- Phase 3 before Phase 4 because the handler calls the router with the tier
- Phase 4 is the integration point -- needs 1+2+3
- Phase 5 is pure observability -- can run in parallel with Phase 4 but ordered after for clarity
- Phase 6 is a minor extension, depends on Phase 3

## Sources

- Codebase analysis: `src/router/selector.rs`, `src/proxy/handlers.rs`, `src/proxy/server.rs`, `src/config.rs`, `src/proxy/types.rs`, `src/storage/logging.rs`
- `.planning/PROJECT.md` for scope, constraints, and key decisions
- Architecture patterns derived from existing codebase conventions (handler-level integration, config-driven behavior, circuit breaker composition)
