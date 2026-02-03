# Feature Landscape: LLM Proxy Reliability and Observability

**Domain:** LLM proxy/gateway reliability and observability
**Researched:** 2026-02-02
**Confidence:** MEDIUM (based on training data knowledge of LiteLLM, Portkey, Helicone, BricksLLM as of early 2025; web verification unavailable)

## Competitive Landscape Summary

The LLM gateway/proxy space has converged on a common feature set. The four products studied -- LiteLLM, Portkey, Helicone, and BricksLLM -- share most reliability and observability features, differing primarily in deployment model (self-hosted vs. SaaS), depth of analytics, and enterprise features. arbstr's unique position (Bitcoin-native, local-first, Routstr marketplace) means some enterprise features are irrelevant, but the core reliability and logging patterns are universal.

| Product | Deployment | Focus | Key Differentiator |
|---------|-----------|-------|--------------------|
| LiteLLM | Self-hosted proxy | Multi-provider routing | 100+ provider support, cost tracking |
| Portkey | SaaS gateway | Reliability + observability | Conditional routing, guardrails |
| Helicone | SaaS layer | Observability + analytics | Deep logging, prompt analytics |
| BricksLLM | Self-hosted proxy | Cost control + access mgmt | Per-key spend limits, rate limiting |

---

## Table Stakes

Features users expect. Missing any of these means the proxy feels broken or incomplete for a reliability/observability milestone.

| Feature | Why Expected | Complexity | Confidence | Notes |
|---------|--------------|------------|------------|-------|
| **Provider fallback on failure** | Every proxy does this. A proxy that fails when one provider is down provides no value over direct calls. | Medium | HIGH | LiteLLM, Portkey, BricksLLM all have fallback. arbstr currently returns an error on first provider failure. |
| **Configurable retry with backoff** | Transient errors (429, 500, 502, 503, 524) are common with LLM providers. Without retry, users see failures that would self-resolve. | Low | HIGH | All four products support retries. Exponential backoff is standard. Max 2-3 retries is the norm. |
| **Request logging to persistent storage** | Without logging, there is no way to know what happened. Every observability product starts here. | Medium | HIGH | Helicone's entire value prop is logging. arbstr has the schema designed and sqlx in Cargo.toml but nothing implemented. |
| **Token count extraction** | Usage data appears in every OpenAI-compatible response. Not capturing it means no cost tracking, no analytics, nothing downstream. | Low | HIGH | Non-streaming: parse `usage` object from response. Streaming: parse final chunk or `stream_options: {include_usage: true}`. |
| **Cost calculation per request** | The core value of arbstr is cost optimization. Without tracking actual cost per request, you cannot validate the proxy is saving money. | Low | HIGH | Formula: `(input_tokens * input_rate / 1000) + (output_tokens * output_rate / 1000) + base_fee`. Current code only uses output_rate (noted as broken in PROJECT.md). |
| **Latency measurement** | Users need to know if the proxy adds meaningful overhead. Every proxy tracks this. | Low | HIGH | Measure wall-clock time from request receipt to response completion (or first byte for streaming). |
| **Structured error responses** | OpenAI-compatible error format must be maintained even during fallback/retry scenarios. Clients depend on parsing these. | Low | HIGH | arbstr already has this (error.rs with OpenAI-compatible JSON errors). Must maintain through fallback logic. |
| **Health check with provider status** | Current `/health` returns a static "ok". Should reflect whether providers are actually reachable. | Low | MEDIUM | LiteLLM and Portkey expose provider health. For arbstr, even a simple "last known status" per provider is valuable. |

### Implementation Priority for Table Stakes

1. **Request logging + token extraction + cost calculation** -- These three are tightly coupled. Implement together. Logging is the foundation that everything else builds on.
2. **Retry with backoff** -- Simple to implement, high impact. Retry on 429/5xx before attempting fallback.
3. **Provider fallback** -- After retry exhaustion on primary provider, try next cheapest. Requires the router to return an ordered list, not just the top pick.
4. **Latency measurement** -- Instrument the request path. Store in the same log table.
5. **Health check enhancement** -- Track provider success/failure rates from logged data.

---

## Differentiators

Features that go beyond expectations. Not missing = nobody notices, but present = perceived quality. These separate a "useful tool" from a "good tool."

| Feature | Value Proposition | Complexity | Confidence | Notes |
|---------|-------------------|------------|------------|-------|
| **Cost tracking in satoshis with query endpoints** | arbstr's unique value. No other proxy tracks cost in sats. Enables "how much did I spend today on code generation?" queries. | Medium | HIGH | Unique to arbstr/Routstr ecosystem. Simple SQL aggregation endpoints. |
| **Learned token ratios per policy** | Predict cost before seeing the response by learning that "code" requests typically have 1:3 input:output ratio. Enables smarter pre-routing decisions. | Medium | MEDIUM | LiteLLM tracks ratios but doesn't use them for routing decisions. This is genuinely novel for arbstr. |
| **Circuit breaker per provider** | After N consecutive failures, stop trying a provider for a cooldown period. Prevents wasting time and tokens on a down provider. | Medium | HIGH | Portkey and LiteLLM both have circuit breakers. Goes beyond simple retry/fallback. |
| **Streaming token counting** | Parse SSE chunks to count tokens in real-time during streaming, rather than relying on provider's final usage report. | High | MEDIUM | Helicone does this. Complex because you need to parse SSE `data:` lines, handle partial JSON, and count across chunks. |
| **Per-model and per-policy cost breakdown** | "How much did claude-3.5-sonnet cost me this week?" vs "How much did my code_generation policy cost?" | Low | HIGH | Simple SQL GROUP BY on the request log. Very high value for a cost-optimization proxy. |
| **Request metadata via response headers** | Return `x-arbstr-cost-sats`, `x-arbstr-latency-ms`, `x-arbstr-retries` headers so clients can see per-request metrics without querying the API. | Low | HIGH | Portkey and Helicone use custom response headers extensively. arbstr already adds `x-arbstr-provider`. |
| **Timeout configuration per provider** | Different providers have different latency profiles. Allow per-provider timeout configuration rather than a global 120s. | Low | MEDIUM | LiteLLM supports per-model timeouts. arbstr currently has a global 120s timeout on the HTTP client. |
| **Stream error detection and clean signaling** | Detect mid-stream provider failures (connection drop, malformed SSE) and either retry transparently or signal the client with a clean error event. | High | MEDIUM | This is hard. Most proxies just drop the connection. Clean mid-stream error signaling (sending an SSE error event) is rare and would be genuinely useful. |

### Differentiator Priority

1. **Cost tracking endpoints** -- Low complexity, high arbstr-specific value.
2. **Response metadata headers** -- Trivial to add during logging implementation.
3. **Per-model/per-policy breakdown** -- Free once logging exists.
4. **Circuit breaker** -- Natural extension of fallback logic.
5. **Learned token ratios** -- Interesting but can wait; needs data first.

---

## Anti-Features

Features to deliberately NOT build in this milestone. Common in the LLM gateway space but wrong for arbstr's context.

| Anti-Feature | Why Other Products Have It | Why arbstr Should NOT Build It | What to Do Instead |
|--------------|---------------------------|-------------------------------|--------------------|
| **Web dashboard UI** | Helicone, Portkey, LiteLLM all have rich web UIs for log browsing, cost charts, and analytics. | arbstr is a single-user local proxy. A web UI adds frontend complexity (JS framework, build pipeline, CORS) for one person who can `curl` or use `sqlite3`. PROJECT.md explicitly excludes this. | Expose JSON query endpoints. Let the user pipe to `jq` or connect any SQLite viewer. |
| **Per-API-key rate limiting** | BricksLLM's core feature. Portkey does this too. | arbstr runs on a home network for one user. There is no abuse vector. Rate limiting adds complexity with zero value. | Skip entirely. If multi-user comes later, add it then. |
| **Client authentication / API key management** | Every SaaS gateway authenticates clients. BricksLLM and LiteLLM have extensive key management. | Same reason. One user, home network. Adding auth means every client config needs an arbstr API key for no security benefit (local network). | Skip. Bind to 127.0.0.1 (already the default). |
| **Prompt caching / semantic cache** | Portkey and LiteLLM offer response caching to avoid duplicate LLM calls. | Caching LLM responses is useful for high-volume production but adds significant complexity (cache invalidation, storage, similarity matching). Single-user usage patterns rarely produce exact duplicate prompts. | Skip. If needed later, implement as a separate middleware layer. |
| **Guardrails / content filtering** | Portkey has input/output guardrails (PII detection, toxicity filtering). | arbstr is a personal tool. The user is responsible for their own prompts. Adding guardrails between yourself and your own proxy is unnecessary friction. | Skip entirely. |
| **Multi-tenant virtual keys** | Portkey and BricksLLM map virtual API keys to real provider keys with per-key budgets. | Single user, single set of provider credentials. Virtual key mapping adds indirection with no benefit. | Skip. Use real Cashu tokens directly in config. |
| **Automatic model fallback to different model** | LiteLLM can automatically fall back from gpt-4o to gpt-3.5-turbo. Portkey has conditional model routing. | Dangerous for a cost-optimization proxy. If the user requests claude-3.5-sonnet for a code task, silently routing to gpt-4o-mini changes quality. Fallback should be same-model-different-provider, not model substitution. | Fallback to same model on different provider only. Let the user explicitly configure model alternatives in policy rules if they want cross-model fallback. |
| **Real-time streaming analytics** | Helicone processes streaming tokens in real-time for live dashboards. | Complexity far exceeds value for single user. Parsing SSE in real-time, maintaining state machines, counting tokens mid-stream -- all for metrics you will look at hours later. | Count tokens from the final response/usage object. If streaming, optionally use `stream_options: {include_usage: true}` to get a final usage chunk. Do not parse every chunk. |
| **Webhook / alerting on spend thresholds** | BricksLLM and Portkey support spend alerts. | A single user running a local proxy does not need automated alerts. They can query cost endpoints after a session. | Provide a CLI command or endpoint for cost queries. No push notifications needed. |

---

## Feature Dependencies

```
Request Logging (SQLite)
  |
  +-- Token Count Extraction (from response)
  |     |
  |     +-- Cost Calculation per Request
  |           |
  |           +-- Cost Query Endpoints (aggregation)
  |           |
  |           +-- Per-Model / Per-Policy Breakdown
  |           |
  |           +-- Learned Token Ratios
  |
  +-- Latency Measurement
  |
  +-- Provider Health Tracking (from success/failure logs)

Retry with Backoff
  |
  +-- Provider Fallback (after retry exhaustion)
        |
        +-- Circuit Breaker (track consecutive failures)

Stream Error Handling (independent but interacts with logging)
```

Key dependency insight: **Logging is the foundation.** Cost tracking, health monitoring, and learned ratios all require logged data. Build logging first, then layer analytics on top.

Retry and fallback are independent of logging and can be implemented in parallel.

---

## Feature Comparison Matrix: What Competitors Offer

| Feature | LiteLLM | Portkey | Helicone | BricksLLM | arbstr (current) | arbstr (target) |
|---------|---------|---------|----------|-----------|-------------------|-----------------|
| Retry with backoff | Yes | Yes | N/A (obs only) | Yes | No | **Yes** |
| Provider fallback | Yes | Yes | N/A | Yes | No | **Yes** |
| Circuit breaker | Yes | Yes | N/A | No | No | **Maybe (differentiator)** |
| Request logging | Yes | Yes | Yes (core) | Yes | No | **Yes** |
| Token counting | Yes | Yes | Yes | Yes | No | **Yes** |
| Cost tracking | Yes (USD) | Yes (USD) | Yes (USD) | Yes (USD) | No | **Yes (sats)** |
| Cost query API | Yes | Yes (dashboard) | Yes (dashboard) | Yes | No | **Yes (JSON endpoints)** |
| Streaming token count | Partial | Yes | Yes | No | No | **No (not needed)** |
| Per-model breakdown | Yes | Yes | Yes | Yes | No | **Yes** |
| Response headers | No | Yes | Yes | No | Partial (provider only) | **Yes (cost, latency)** |
| Web dashboard | Yes | Yes | Yes | Yes | No | **No (deliberate)** |
| API key management | Yes | Yes | No | Yes | No | **No (deliberate)** |
| Prompt caching | Yes | Yes | No | No | No | **No (deliberate)** |

---

## MVP Recommendation for This Milestone

Based on the competitive landscape and arbstr's specific context (single-user, cost-optimization focus, existing codebase), here is the recommended feature set for the reliability and observability milestone.

### Must Have (Table Stakes)

1. **Retry with exponential backoff** -- Retry on 429, 500, 502, 503, 504. Max 2 retries. Configurable but sensible defaults.
2. **Provider fallback on failure** -- After retries exhausted, try next cheapest provider for the same model. Return error only when all providers fail.
3. **Request logging to SQLite** -- Every request logged with: timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success, policy name.
4. **Token count extraction** -- Non-streaming: parse `usage` from response body. Streaming: request `stream_options: {include_usage: true}` or accept null tokens for streams initially.
5. **Fix cost calculation** -- Use full formula: `(input_tokens * input_rate / 1000) + (output_tokens * output_rate / 1000) + base_fee`.
6. **Latency measurement** -- Wall-clock time per request, stored in log table.
7. **Stream error handling** -- Detect connection drops and malformed responses. Do not attempt transparent retry mid-stream (too complex). Signal error cleanly to client.

### Should Have (Differentiators worth the effort)

8. **Cost query endpoint** -- `GET /costs?period=today|week|month` returning total sats spent, with optional model/policy grouping.
9. **Response metadata headers** -- `x-arbstr-cost-sats`, `x-arbstr-latency-ms`, `x-arbstr-retries` on every response.
10. **Enhanced health endpoint** -- `/health` returns per-provider last-known status and success rate.

### Defer to Later Milestone

11. **Circuit breaker** -- Needs more operational data to tune thresholds. Add after logging is generating data.
12. **Learned token ratios** -- Needs logged data to learn from. Add after a period of data collection.
13. **Per-provider timeout configuration** -- Nice to have but global 120s works for now.

---

## Complexity Estimates

| Feature | Lines of Code (est.) | New Files | Touches Existing | Risk |
|---------|---------------------|-----------|-----------------|------|
| SQLite setup + migrations | ~100 | storage/mod.rs, storage/db.rs | server.rs (state) | Low -- sqlx already in deps |
| Request logging | ~150 | storage/logger.rs | handlers.rs | Low -- straightforward insert |
| Token extraction (non-streaming) | ~50 | None (in handlers) | handlers.rs | Low -- parse usage object |
| Cost calculation fix | ~30 | None | selector.rs, handlers.rs | Low -- arithmetic |
| Retry with backoff | ~120 | proxy/retry.rs or in handlers | handlers.rs | Medium -- async retry loop |
| Provider fallback | ~80 | None (modify selector + handler) | selector.rs, handlers.rs | Medium -- need ordered provider list |
| Stream error handling | ~80 | None (in handlers) | handlers.rs | Medium -- error detection in stream |
| Cost query endpoint | ~100 | New handler | handlers.rs, server.rs | Low -- SQL aggregation |
| Response headers | ~30 | None | handlers.rs | Low -- add headers |
| Health enhancement | ~60 | None | handlers.rs, state | Low -- query log table |

**Total estimated new/changed code:** ~800 lines

---

## Sources and Confidence Notes

All findings are based on training data knowledge of these products as of early 2025. Web verification was attempted but unavailable.

- **LiteLLM**: Well-known open-source project (github.com/BerriAI/litellm). Features documented at docs.litellm.ai. HIGH confidence on core features (fallback, retry, routing, cost tracking) as these have been stable for 1+ years.
- **Portkey**: Commercial AI gateway (portkey.ai). MEDIUM confidence -- feature set was expanding rapidly; specifics may have changed.
- **Helicone**: Observability-focused SaaS (helicone.ai). HIGH confidence on logging and cost tracking features as these are the core product.
- **BricksLLM**: Open-source proxy (github.com/bricks-cloud/BricksLLM). MEDIUM confidence -- smaller project, less certain about current feature state. Note: project may have been renamed or reorganized since training data.
- **Feature categorization (table stakes vs differentiators)**: HIGH confidence. The convergence across products is clear -- fallback, retry, logging, and cost tracking are universal. The distinctions are based on consistent patterns across all four products.
