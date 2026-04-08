# Project Research Summary

**Project:** arbstr v1.7 — Prompt Complexity Scoring and Tier-Aware Routing
**Domain:** Heuristic LLM prompt classification and tier-based provider routing
**Researched:** 2026-04-08
**Confidence:** HIGH

## Executive Summary

arbstr v1.7 adds heuristic complexity scoring to route prompts to appropriately capable providers — local models for simple queries, standard for moderate tasks, frontier models for complex reasoning. This is a well-researched problem: RouteLLM, NVIDIA's DeBERTa classifier, Portkey, LiteLLM, and Requesty all implement variants of this pattern. The consensus approach is a weighted-sum heuristic with 4-6 signals (context length, code blocks, reasoning keywords, conversation depth, system prompt complexity), with configurable thresholds mapping scores to tiers. The stack footprint is minimal: one direct dependency addition (`regex` crate, which is already transitive), leveraging all existing Rust/axum/SQLite infrastructure.

The recommended implementation is a pure-function scorer in a new `src/scorer/` module, called inline in the handler after request parsing. Provider configs gain an optional `tier` field (defaulting to `standard` for backward compatibility), and the router gains tier filtering before its existing cost-sort. Tier escalation — when a scored tier's providers are all circuit-broken — is implemented as candidate-list expansion (one-way, no cycling) at the handler level. This slots cleanly into the existing `resolve_candidates` flow.

The primary risk is false negatives: complex prompts misclassified as simple, routed to local models that produce plausible-but-wrong output. The mitigation is conservative defaults — route to frontier when uncertain, only route down when MULTIPLE simple signals agree. A secondary risk is latency: the scorer runs on every request's hot path and must stay under 1ms. Both risks are addressable with disciplined design choices made before implementation begins; they are hard to fix after the fact.

## Key Findings

### Recommended Stack

The existing Rust/axum/SQLite stack requires only one addition: `regex = "1.12"` as a direct dependency (already transitive via `tracing-subscriber`, so net new transitive deps = zero). All other needs are covered by existing deps: `serde`/`toml` for config parsing, `sqlx` for the new DB columns, `tracing` for per-signal debug logging.

Explicitly rejected: `tiktoken-rs` (50ms cold-start tokenizer load, overkill for heuristic bucketing), `linfa` ML classifier (Python/ONNX dependency, 50-200ms latency, out of scope per PROJECT.md), `lazy-regex` (stdlib `LazyLock` covers this since Rust 1.80), and any readability index crates (Flesch-Kincaid measures prose quality, not LLM prompt task complexity).

**Core technologies:**
- `regex` 1.12: pattern matching for code fences, file path references, word-boundary keyword matching — already in dependency tree, zero compile cost
- `std::sync::LazyLock`: compile regex patterns once at startup, store in scorer struct in AppState — stdlib since Rust 1.80, no `once_cell` needed
- `prompt.len() / 4`: token count approximation — O(1), accurate enough for tier bucketing with 2x error tolerance

### Expected Features

**Must have (table stakes):**
- Heuristic complexity scorer (5 signals: reasoning keywords, context length, code blocks, conversation depth, system prompt complexity) — every routing system in the space implements this
- Provider tier field in config (`tier = "local" | "standard" | "frontier"`, default `"standard"`) — required for the router to know what to filter
- Tier-aware provider selection: filter by tier, then cost-sort within tier — the core routing decision
- Configurable thresholds (local_max, standard_max) in `[scoring]` config section — hardcoded thresholds are the top complaint in every routing system
- `X-Arbstr-Tier` header override to bypass scoring — follows existing `X-Arbstr-Policy` precedent, required escape hatch for misclassifications
- `x-arbstr-complexity-score` and `x-arbstr-tier` response headers — observability is non-negotiable
- `complexity_score` and `tier` columns in `requests` DB table — required for stats analysis
- Tier escalation on circuit break — without this, all local-tier providers being down causes 503s instead of graceful degradation

**Should have (differentiators):**
- Configurable signal weights in `[scoring.weights]` — lets users adapt to their workload distribution
- Conversation depth as a distinct signal — no other heuristic router tracks this; arbstr sees the full messages array
- `stats?group_by=tier` — tier-level cost analytics not available in any other local proxy
- Cost endpoint tier awareness — `POST /v1/cost` shows "this prompt would route to local tier at X sats"
- Per-policy tier overrides (`allowed_tiers` on policy rules) — prevents code generation from hitting local models
- `x-arbstr-escalated: true` header — makes escalation events visible to clients

**Defer beyond v1.7:**
- Downgrade on budget pressure (vault balance drives tier bias) — high complexity, unclear UX, vault integration risk
- Configurable keyword sets (ship hardcoded defaults, make configurable only if users request it)
- ML-based classifier — explicitly out of scope per PROJECT.md, can be added later behind the scorer trait boundary
- Response-quality cascading — doubles cost on escalation, inappropriate for a transparent proxy

### Architecture Approach

The complexity scorer is a pure function in `src/scorer/mod.rs`, called inline in the `chat_completions` handler after request body parsing but before `resolve_candidates`. It receives the full `ChatCompletionRequest` and returns a `ComplexityScore { score: f32, tier: Tier, signals: SignalBreakdown }`. The tier value flows into a modified `resolve_candidates_with_tier` which builds an escalation-ordered candidate list (scored-tier providers first, then higher tiers if circuits are all open). This is composition, not middleware — the scorer needs the parsed request body, which is unavailable to axum middleware layers.

**Major components:**
1. `src/scorer/mod.rs` (NEW) — pure function scorer, 6 heuristic signals, weighted sum, tier mapping via thresholds
2. `src/config.rs` additions (MODIFIED) — `ScorerConfig`, `SignalWeights`, `TierThresholds`, `tier` field on `ProviderConfig`
3. `src/router/selector.rs` (MODIFIED) — `select_candidates` gains `max_tier: Option<Tier>` filter parameter
4. `src/proxy/handlers.rs` (MODIFIED) — call scorer, parse `X-Arbstr-Tier` header, implement `resolve_candidates_with_tier` with one-way escalation
5. DB migration (NEW) — `complexity_score REAL` and `tier TEXT` columns on `requests` table

**Build order (dependency-driven):** Tier enum + ProviderConfig field first (everything depends on the Tier type), then scorer pure logic (independently testable), then router tier filtering, then handler integration + escalation, then DB observability, then cost endpoint update.

### Critical Pitfalls

1. **False negatives route complex prompts to local models** — Default to frontier when uncertain. Only route down when MULTIPLE simple signals agree. Conservative bias from day one; wrong downgrades are strictly worse than wasted frontier calls.

2. **Scoring only the last message misses conversation context** — Scorer function signature must be `score(messages: &[Message], ...)` not `score(prompt: &str, ...)`. Conversation depth (`messages.len()`) is a first-class signal. Hard to fix after call sites are established.

3. **Scorer latency on hot path** — 1ms budget, enforced by benchmark test. Use `str::contains()` not compiled regex for keyword matching on small keyword sets. Short-circuit on byte-length (>32KB = complex). No tokenization, no lowercase copies of full prompts.

4. **Tier escalation cycling creates infinite retry loops** — One-way gate: once escalated, never de-escalate within the same request. Implement as candidate-list expansion (local + standard + frontier in order), not as re-routing. Score is immutable per request.

5. **Config complexity explosion** — One primary user-facing knob (bias: conservative/balanced/aggressive). Individual signal weights are tunable but secondary. `[scoring]` section fully optional; zero-config produces balanced behavior.

6. **`tier` field breaks existing configs without `#[serde(default)]`** — Must default to `Tier::Standard`. Existing configs parse unchanged. Write migration test as part of Phase 1.

## Implications for Roadmap

### Phase 1: Tier Type and Provider Config
**Rationale:** The `Tier` enum is the foundational type used by every other component. Config backward compatibility must be established here, not retrofitted. Zero-risk phase — pure additions, no behavioral changes.
**Delivers:** `Tier` enum with `Ord` (Local < Standard < Frontier), `tier` field on `ProviderConfig` with `#[serde(default)]`, `tier` on `SelectedProvider`, updated `config.example.toml`.
**Addresses:** Provider tier assignment (table stakes)
**Avoids:** Pitfall 6 (config breakage) — migration test written in this phase

### Phase 2: Complexity Scorer (Pure Logic)
**Rationale:** Pure functions with no integration dependencies, independently testable before touching production code paths. The scoring behavior and signal balance must be correct before the routing pipeline depends on it.
**Delivers:** `src/scorer/mod.rs` with `score(messages: &[Message], config: &ScorerConfig)`, all 6 signals, `ScorerConfig`/`SignalWeights`/`TierThresholds` in config, `[scoring]` TOML section, unit tests on sample prompt corpus with 1ms benchmark.
**Addresses:** Heuristic scorer, configurable thresholds and weights (table stakes)
**Avoids:** Pitfall 2 (conversation context) — function signature decided here; Pitfall 3 (latency) — benchmark established here; Pitfall 5 (config explosion) — optional section, balanced defaults

### Phase 3: Router Tier Filtering
**Rationale:** Isolated change to `selector.rs` — adds one parameter and one filter. Independently unit-testable with mock providers. Establishes the tier-filtering contract before handler integration.
**Delivers:** `select_candidates` gains `max_tier: Option<Tier>`, tier filtering (`p.tier <= max_tier`), `Error::NoTierMatch`, all existing call sites pass `None` (backward compatible).
**Addresses:** Tier-aware provider selection (table stakes)
**Avoids:** Cost-sort within tier preserved; NoTierMatch returned to caller, not handled inside router (enables clean escalation in Phase 4)

### Phase 4: Handler Integration and Escalation
**Rationale:** Integration phase — depends on all prior phases. Highest-risk: escalation + circuit breaker interaction. One-way candidate-list expansion pattern prevents the cycling pitfall but must be implemented carefully.
**Delivers:** Scorer called in handler, `resolve_candidates_with_tier` with one-way escalation, `X-Arbstr-Tier` header override, `x-arbstr-complexity-score` / `x-arbstr-tier` / `x-arbstr-escalated` response headers, SSE trailing metadata updated.
**Addresses:** Header override, response headers, escalation on circuit break (all table stakes)
**Avoids:** Pitfall 1 (false negatives) — conservative default in handler; Pitfall 4 (cycling) — candidate-list expansion, score immutable per request

### Phase 5: Observability (DB and Stats)
**Rationale:** Pure observability additions after routing is live. No behavioral changes. Enables threshold tuning based on real traffic distribution.
**Delivers:** DB migration (`complexity_score REAL`, `tier TEXT`), `RequestLog` updated, INSERT/UPDATE queries updated, `stats?group_by=tier`.
**Addresses:** Complexity/tier in DB logging (table stakes), stats group_by=tier (differentiator)

### Phase 6: Cost Endpoint Update
**Rationale:** Minor extension of existing `/v1/cost` handler. Low risk, independent of Phase 5. Useful for users wanting to understand routing economics before sending a request.
**Delivers:** `POST /v1/cost` accepts optional tier parameter and returns tier-filtered cost estimate.
**Addresses:** Cost estimation with tier awareness (differentiator)

### Phase Ordering Rationale

- Phase 1 must be first: `Tier` is used by scorer, router, handler, and DB — nothing else can compile without it
- Phase 2 before Phase 3: scorer produces the `Tier` value that the router consumes as input
- Phase 3 before Phase 4: the handler calls the router with the tier, so router API must exist first
- Phase 4 is the integration gate: only meaningful end-to-end after phases 1+2+3 are complete and tested
- Phase 5 after Phase 4: DB columns should store real routing decisions, not placeholder values
- Phase 6 last: lowest risk, no blocking dependencies after Phase 3

### Research Flags

Phases with well-documented patterns (skip research-phase):
- **Phase 1:** Serde default patterns in Rust are established; `Ord` on enums is standard
- **Phase 2:** Weighted-sum heuristic scorer with 5-6 signals has known implementation patterns from research
- **Phase 3:** Adding an optional filter parameter to an existing selection function is a minimal change
- **Phase 5:** SQLite ALTER TABLE migration and GROUP BY query patterns already established in this codebase
- **Phase 6:** Extends existing `/v1/cost` endpoint using same patterns as Phase 3

Phase warranting careful pre-implementation review:
- **Phase 4:** Escalation + existing circuit breaker + existing retry loop interaction is the highest-risk integration. Recommend reviewing `handlers.rs`, `retry.rs`, and `circuit_breaker.rs` together before writing code. The one-way gate and candidate-list expansion pattern is clear, but the exact insertion point in the existing `resolve_candidates` flow needs tracing carefully.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Verified via `cargo tree` (regex already transitive), `cargo search` for current versions, explicit rejection of alternatives with rationale |
| Features | HIGH | Cross-referenced against 10+ production routing systems; signal weights validated against NVIDIA DeBERTa formula and RouteLLM research |
| Architecture | HIGH | Derived from direct codebase analysis of all relevant source files; integration points explicitly located in selector.rs and handlers.rs |
| Pitfalls | HIGH | Based on codebase analysis + production LLM routing system patterns + heuristic classifier anti-patterns from search/ranking literature |

**Overall confidence:** HIGH

### Gaps to Address

- **Default threshold calibration (local_max=0.3, standard_max=0.7):** These values are drawn from analogical reasoning against NVIDIA's formula and RouteLLM calibration but have not been validated against real arbstr traffic. Ship conservative (bias toward frontier) and plan to tune after collecting distribution data via `stats?group_by=tier`.
- **Keyword set completeness:** The default reasoning keyword list is research-derived but unvalidated against real user prompts. Config should allow adding keywords (`extra_keywords`) rather than requiring full replacement, so defaults can be updated without breaking user configs.
- **Vault reservation under escalation:** When tier escalation is possible (local-tier request might escalate to frontier), vault reservation must use frontier-tier pricing (worst case) to avoid under-reservation rejections. This `vault.rs` interaction needs explicit design during Phase 4 planning — it is flagged in PITFALLS.md but not fully resolved in ARCHITECTURE.md.

## Sources

### Primary (HIGH confidence)
- Direct codebase analysis: `src/router/selector.rs`, `src/proxy/handlers.rs`, `src/proxy/circuit_breaker.rs`, `src/proxy/retry.rs`, `src/config.rs`, `src/storage/logging.rs`
- `cargo tree` output — confirmed `regex` 1.12.3 already transitive via `tracing-subscriber`
- `.planning/PROJECT.md` — scope, constraints, out-of-scope items

### Secondary (MEDIUM confidence)
- [RouteLLM Blog (LMSYS)](https://www.lmsys.org/blog/2024-07-01-routellm/) — MF router, 95% GPT-4 quality at 48% cost reduction
- [NVIDIA Prompt Task and Complexity Classifier](https://huggingface.co/nvidia/prompt-task-and-complexity-classifier) — 6-dimension weighted scoring formula, DeBERTa backbone, 98% accuracy
- [Portkey Conditional Routing](https://portkey.ai/docs/product/ai-gateway/conditional-routing) — runtime routing on request params and metadata
- [LiteLLM Auto Routing](https://docs.litellm.ai/docs/proxy/auto_routing) — embedding-based semantic matching (evaluated and rejected)
- [LLM Routing in Production (LogRocket)](https://blog.logrocket.com/llm-routing-right-model-for-requests/) — input length as complexity proxy, keyword classification
- [Requesty Intelligent Routing](https://www.requesty.ai/blog/intelligent-llm-routing-in-enterprise-ai-uptime-cost-efficiency-and-model) — hybrid local/cloud routing, 30-70% cost reduction

### Tertiary (LOW confidence)
- [Complexity-Based Prompting (ICLR 2023)](https://openreview.net/pdf?id=yf1icZHC-l9) — reasoning steps as primary complexity factor (academic, production applicability uncertain)
- [SLM-default LLM-fallback Pattern](https://www.strathweb.com/2025/12/slm-default-llm-fallback-pattern-with-agent-framework-and-azure-ai-foundry/) — confidence-gated escalation pattern (evaluated and rejected as anti-feature for transparent proxy)

---
*Research completed: 2026-04-08*
*Ready for roadmap: yes*
