# Feature Landscape: Intelligent Complexity Routing

**Domain:** LLM prompt complexity classification and tier-aware routing
**Researched:** 2026-04-08
**Context:** Adding to existing Rust proxy (arbstr) with cost-based routing, circuit breakers, retry/fallback, SQLite logging, and vault treasury integration already shipped.

## Table Stakes

Features that any complexity-aware LLM router is expected to have. Missing any of these and the routing feels broken or useless.

| Feature | Why Expected | Complexity | Dependencies on Existing |
|---------|--------------|------------|--------------------------|
| Heuristic complexity scorer | Core of the feature -- without scoring, no tier routing happens. Every router in the space (RouteLLM, Portkey, Requesty) classifies prompts before routing. | Medium | None -- new module |
| Provider tier assignment | Users must tag providers as local/standard/frontier. Without tiers, the scorer has nothing to route to. Portkey, LiteLLM, and Requesty all tier their providers. | Low | Extends `ProviderConfig` in config.rs |
| Tier-aware provider selection | Router must filter by tier before cost-optimizing within the tier. This is the routing decision itself. | Medium | Extends `Router::select_provider` in selector.rs |
| Configurable complexity thresholds | What score separates local from standard from frontier? Must be tunable per deployment. Hardcoded thresholds are the number one complaint in every routing system. | Low | New fields in config.toml `[complexity]` section |
| Header override for complexity tier | Power users need `X-Arbstr-Tier: frontier` to bypass scoring. Portkey has this via metadata routing. arbstr already has `X-Arbstr-Policy` as precedent. | Low | Handler extraction in handlers.rs |
| Complexity score in response headers | Observability is non-negotiable. Users need `x-arbstr-complexity-score` and `x-arbstr-tier` to understand routing decisions. arbstr already sets cost/latency headers. | Low | Extends header injection in handlers.rs |
| Complexity/tier in DB logging | Must persist for analytics. Without this, stats endpoint cannot report by tier. arbstr already logs model/provider/cost/latency. | Low | New columns in `requests` table via migration |
| Escalation on circuit break | If the selected tier's providers are all circuit-broken, escalate to next tier up. This is where tier routing intersects with existing circuit breaker. Without it, users get 503s when local providers are down instead of graceful degradation. | Medium | Integrates with existing `CircuitBreakerRegistry` |

## Differentiators

Features that set arbstr apart from generic LLM gateways. Not expected, but highly valuable for the use case.

| Feature | Value Proposition | Complexity | Dependencies on Existing |
|---------|-------------------|------------|--------------------------|
| Configurable signal weights | Let users tune which signals matter: `context_length_weight = 0.3, code_weight = 0.2, reasoning_weight = 0.3, keyword_weight = 0.2`. NVIDIA's classifier uses fixed weights (0.35/0.25/0.15/0.15/0.05/0.05). Making these configurable lets users adapt to their workload without code changes. | Low | New `[complexity.weights]` in config.toml |
| Conversation depth signal | Multi-turn conversations with many messages are harder than single-shot. No heuristic router in the ecosystem tracks this -- they all treat each request independently. arbstr sees the full messages array and can exploit it. | Low | Computed from `messages.len()` in request body |
| Cost estimation with tier awareness | Existing `/v1/cost` endpoint should factor in which tier a prompt would route to. "This prompt is simple, it would cost X sats on local" vs "This prompt is complex, it would cost Y sats on frontier." | Low | Extends existing `/v1/cost` handler |
| Stats group_by=tier | Analytics broken down by tier: "How much am I spending on frontier vs local?" Extends existing `/v1/stats` with a new group_by dimension. No other local proxy offers tier-level cost analytics. | Medium | Extends existing stats.rs query builder |
| Downgrade on budget pressure | When vault balance is low, bias thresholds toward cheaper tiers. Integrates complexity routing with existing vault treasury in a way no other system does (they do not have Bitcoin billing). | High | Integrates with vault.rs balance checks |
| Per-policy tier overrides | Policy rules should optionally constrain which tiers are eligible. `[[policies.rules]]` with `allowed_tiers = ["standard", "frontier"]` to prevent code generation from ever hitting local models. | Low | Extends `PolicyRule` in config.rs |

## Anti-Features

Features to explicitly NOT build for v1.7. These are tempting but wrong for this milestone.

| Anti-Feature | Why Avoid | What to Do Instead |
|--------------|-----------|-------------------|
| ML-based classifier (BERT, matrix factorization) | RouteLLM's MF router needs Python, training data, and model weights. arbstr is a Rust proxy targeting less than 10ms overhead per routing decision. ML classifiers add 50-200ms latency and a Python/ONNX dependency. PROJECT.md explicitly lists "ML-based policy classification" as out of scope. | Use deterministic heuristic scoring. Fast, predictable, debuggable. The scoring interface (trait) can support ML backends later without changing the routing pipeline. |
| LLM-as-judge for complexity | Using a cheap LLM to classify whether to use a better LLM is a recursive cost problem. Adds latency (200-500ms minimum), requires a working provider just to route, and creates a chicken-and-egg failure mode if providers are down. | Heuristic scoring runs in less than 1ms with zero external calls. |
| Response-quality-based cascading | True cascading (send to cheap model first, check quality, escalate if bad) requires running inference twice. Doubles cost on escalation and adds significant latency. RouteLLM and academic cascade papers show this works but at complexity and cost that is inappropriate for a local proxy. | Pre-route based on prompt analysis only. Escalation should only happen on provider failure (circuit break), not on quality assessment. |
| Semantic embedding similarity | LiteLLM's auto-routing uses embeddings to match queries against reference prompts. Requires an embedding model running locally or API calls for every request. Overkill for a single-user local proxy. | Keyword matching (already exists in policy engine) plus structural heuristics (code blocks, message count, token length). |
| Automatic threshold tuning | Self-adjusting thresholds based on historical data sounds smart but makes routing unpredictable. Users cannot reason about why a prompt was routed differently today versus yesterday. | Explicit thresholds in config.toml. Users tune manually using `/v1/stats?group_by=tier` data to see distribution and cost impact. |
| Multi-model response comparison | Running same prompt on multiple models and picking best response. Extremely expensive, defeats the purpose of cost optimization entirely. | Single-path routing with deterministic tier selection. |
| Self-reported confidence scoring | The SLM-default LLM-fallback pattern has the first model rate its own confidence and escalate on low scores. Requires running inference before deciding to escalate, adding latency to every request. Works for chat agents, wrong for a transparent proxy. | Score complexity from the prompt before any inference happens. |

## Heuristic Complexity Signals

Based on research across RouteLLM, NVIDIA's prompt classifier, Portkey, Requesty, and the academic literature, these are the signals that work for rule-based classification without ML models.

### Signals to Implement (ordered by predictive value)

| Signal | What It Detects | How to Compute | Default Weight |
|--------|----------------|----------------|----------------|
| **Reasoning keywords** | Prompts requiring logical/analytical effort ("analyze", "compare", "evaluate", "explain why", "prove", "debug", "step by step", "think through", "trade-offs") | Keyword match against configurable set in last user message. Normalize: 0 matches = 0.0, 3+ matches = 1.0. | 0.30 |
| **Context length** | Large inputs correlate with harder tasks. Research confirms input length is "a decent proxy for complexity." LogRocket and NVIDIA both use it. | Total character count across all messages divided by 4 (rough token estimate). Normalize: 0-200 tokens = 0.0, 4000+ tokens = 1.0, linear between. | 0.25 |
| **Code block presence** | Code generation/analysis requires capable models. NVIDIA classifies "Code Generation" as a distinct high-complexity task type. | Count fenced code blocks (triple-backtick markers) in all messages. Normalize: 0 blocks = 0.0, 3+ blocks = 1.0. | 0.20 |
| **Conversation depth** | Multi-turn conversations accumulate context and require coherence tracking across exchanges. | `messages.len()` counting user+assistant pairs. Normalize: 1 message = 0.0, 10+ messages = 1.0. | 0.15 |
| **System prompt complexity** | Long/detailed system prompts indicate specialized tasks requiring instruction-following capability. | System message character count / 4. Normalize: 0-100 tokens = 0.0, 1000+ tokens = 1.0. | 0.10 |

### Signal Scoring Approach

Each signal produces a normalized score in the range 0.0 to 1.0. The final complexity score is a weighted sum:

```
complexity = sum(signal_score_i * weight_i) for all signals
```

Map to tiers via configurable thresholds:
```
complexity < local_threshold     -> local tier      (default: 0.3)
complexity < frontier_threshold  -> standard tier   (default: 0.7)
complexity >= frontier_threshold -> frontier tier
```

This matches the approach used by NVIDIA's classifier (weighted sum of dimension scores) but uses deterministic heuristics instead of ML inference. The NVIDIA formula weights creativity at 0.35, reasoning at 0.25, constraints at 0.15, domain knowledge at 0.15, contextual knowledge at 0.05, and few-shots at 0.05. Our weights differ because we optimize for routing decisions (is this prompt too hard for a local model?) rather than taxonomy (what kind of task is this?).

### Scoring Interface Design

The scorer should be a trait so that the heuristic implementation can be swapped for ML-based scoring later without changing the routing pipeline:

```
trait ComplexityScorer {
    fn score(&self, request: &ChatCompletionRequest) -> ComplexityResult;
}

struct ComplexityResult {
    score: f64,        // 0.0 - 1.0
    tier: Tier,        // Local / Standard / Frontier
    signals: HashMap<String, f64>,  // Per-signal breakdown for observability
}
```

## Feature Dependencies

```
Config: tier field on ProviderConfig
  |
  +--> Complexity scorer module (new src/complexity/)
  |      |
  |      +--> Signal extractors (keyword, length, code, depth, system prompt)
  |      |
  |      +--> Weighted sum combiner with configurable weights
  |      |
  |      +--> Tier mapping from score via thresholds
  |
  +--> Tier-aware router (extends selector.rs)
  |      |
  |      +--> Filter providers by tier from score
  |      |
  |      +--> Cost-optimize within tier (existing logic)
  |      |
  |      +--> Escalation on circuit break (extends circuit_breaker integration)
  |
  +--> Header override parsing (X-Arbstr-Tier)
  |
  +--> Response headers (x-arbstr-complexity-score, x-arbstr-tier)
  |
  +--> DB migration (complexity_score REAL, tier TEXT columns)
  |      |
  |      +--> Stats group_by=tier
  |
  +--> Config: [complexity] section with thresholds and weights
```

## MVP Recommendation

**Phase 1 -- Core scoring and tier routing:**
1. Provider tier field in config (`tier = "local" / "standard" / "frontier"`, default "standard")
2. Heuristic complexity scorer with 5 signals (reasoning keywords, context length, code blocks, conversation depth, system prompt complexity)
3. Configurable thresholds and signal weights in `[complexity]` config section
4. Tier-aware provider selection: score prompt, pick tier, cost-optimize within tier
5. Escalation on circuit break to next tier up

**Phase 2 -- Observability:**
6. Response headers: `x-arbstr-complexity-score`, `x-arbstr-tier`
7. Trailing SSE metadata includes complexity score and tier
8. DB migration: `complexity_score REAL`, `tier TEXT` columns on requests table
9. `X-Arbstr-Tier` header override to bypass scoring

**Phase 3 -- Analytics:**
10. Stats `group_by=tier` support
11. Cost endpoint tier awareness
12. Per-policy tier overrides (`allowed_tiers` field on policy rules)

**Defer beyond v1.7:**
- Downgrade on budget pressure (high complexity, vault integration risk, unclear UX)
- Configurable keyword sets (start with hardcoded reasonable defaults, make configurable if users ask)

## Sources

- [RouteLLM Blog (LMSYS)](https://www.lmsys.org/blog/2024-07-01-routellm/) -- Matrix factorization router, preference-based training, 95% GPT-4 quality at 48% cost reduction
- [RouteLLM GitHub](https://github.com/lm-sys/RouteLLM) -- MF router recommended as strong+lightweight, calculate_strong_win_rate interface
- [NVIDIA Prompt Task and Complexity Classifier](https://huggingface.co/nvidia/prompt-task-and-complexity-classifier) -- 6-dimension weighted scoring formula, DeBERTa backbone, 11 task types, 98% accuracy
- [Martian Model Router](https://route.withmartian.com/) -- Commercial router predicting model behavior from internals
- [Portkey Conditional Routing](https://portkey.ai/docs/product/ai-gateway/conditional-routing) -- Runtime routing on request params and metadata
- [LiteLLM Auto Routing](https://docs.litellm.ai/docs/proxy/auto_routing) -- Embedding-based semantic matching for model selection
- [Top 5 LLM Routing Techniques (Maxim)](https://www.getmaxim.ai/articles/top-5-llm-routing-techniques/) -- Semantic, cost-aware, intent-based, cascading, load balancing
- [SLM-default LLM-fallback Pattern](https://www.strathweb.com/2025/12/slm-default-llm-fallback-pattern-with-agent-framework-and-azure-ai-foundry/) -- Confidence-gated escalation from small to large models, self-reported scoring
- [Unified Routing and Cascading (ETH Zurich)](https://files.sri.inf.ethz.ch/website/papers/dekoninck2024cascaderouting.pdf) -- Academic framework unifying routing and cascading, 16x efficiency gains
- [LLM Routing in Production (LogRocket)](https://blog.logrocket.com/llm-routing-right-model-for-requests/) -- Input length as complexity proxy, keyword-based classification, tier triage analogy
- [Not Diamond](https://www.notdiamond.ai/) -- Specialized routing model trained on cross-domain eval data, prompt adaptation
- [Complexity-Based Prompting (ICLR 2023)](https://openreview.net/pdf?id=yf1icZHC-l9) -- Reasoning steps as primary complexity factor over prompt length
- [Requesty Intelligent Routing](https://www.requesty.ai/blog/intelligent-llm-routing-in-enterprise-ai-uptime-cost-efficiency-and-model) -- Hybrid local/cloud routing, 30-70% cost reduction
- [Not Diamond Awesome AI Model Routing](https://github.com/Not-Diamond/awesome-ai-model-routing) -- Curated list of routing approaches and research
