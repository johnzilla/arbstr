# Feature Landscape: Circuit Breaker and Enhanced /health

**Domain:** Per-provider circuit breaker and health reporting for LLM routing proxy
**Researched:** 2026-02-16
**Overall confidence:** HIGH

## Current State Summary

arbstr already has a robust retry-with-fallback system in `src/proxy/retry.rs`:
- Up to 3 attempts (1 initial + 2 retries) on the primary provider with exponential backoff (1s, 2s)
- Single fallback attempt on the next-cheapest provider after primary exhausts retries
- 30-second total timeout wrapping the entire retry+fallback chain
- Retryable errors: 500, 502, 503, 504 (server errors); 4xx errors fail immediately
- Attempt tracking via `Arc<Mutex<Vec<AttemptRecord>>>` that survives timeout cancellation
- Streaming requests do NOT use the retry system (fail fast, no retry)

The current `/health` endpoint returns a static `{"status": "ok", "service": "arbstr"}` with no provider-level information.

**What's missing:** There is no mechanism to remember that a provider has been failing across multiple request lifecycles. Each new request starts fresh -- the retry module has no memory of previous failures. A provider returning 503 on every request will be retried 3 times on every single request, wasting time and bandwidth. The circuit breaker closes this gap by tracking failure history across requests and skipping known-bad providers.

---

## Table Stakes

Features users expect from a circuit breaker in a proxy/gateway. Missing any of these means the circuit breaker is incomplete or misleading.

| Feature | Why Expected | Complexity | Notes |
|---------|--------------|------------|-------|
| 3-state machine (Closed/Open/Half-Open) | Industry standard from Microsoft Azure reference architecture, implemented by Envoy, Kong, KrakenD, Tyk, HAProxy. Anything less is just a blocklist with no recovery path. | Medium | State stored per-provider in shared concurrent structure. Each provider gets an independent circuit. |
| Consecutive failure threshold to trip Open | Simplest correct approach for a local proxy with low traffic volume. Failure-rate sliding windows are overkill here (see Anti-Features). KrakenD uses consecutive errors for their per-backend circuit breaker. | Low | User spec: 3 consecutive failures. Only count retryable errors (5xx, connection timeouts), not 4xx client errors -- consistent with existing `is_retryable()` in retry.rs. |
| Timeout-based transition to Half-Open | Standard recovery mechanism across all implementations. Without it, a tripped provider can never recover until process restart. | Low | User spec: 30s. Use `tokio::time::Instant` for monotonic tracking. Store the instant when Open state was entered; check elapsed time on next routing decision. |
| Single probe request in Half-Open | Allow exactly one request through when timeout expires. Success transitions to Closed, failure transitions back to Open (restart timer). Azure and KrakenD both use this approach. | Medium | Must serialize probe access -- only one in-flight probe per provider at a time. Use a `probe_in_flight: bool` flag or AtomicBool within the Half-Open state. |
| Fail-fast with 503 when all circuits open | When the only available provider(s) have open circuits, return 503 Service Unavailable immediately. This is the standard response code across Envoy, Tyk, HAProxy, and Spring Cloud Gateway for open circuit conditions. | Low | Must use OpenAI-compatible error format (`{"error": {"message": ..., "type": "arbstr_error", "code": 503}}`). New `Error::CircuitOpen` variant. |
| Skip open-circuit providers in routing | Router must exclude providers with open circuits from the candidate list during selection. This is the core value proposition of the circuit breaker. | Medium | Filter in `select_candidates()` after model/policy filtering, before cost sorting. Pass `CircuitBreakerRegistry` reference into the method. |
| Success resets consecutive failure counter | When a request succeeds on a provider, its failure counter resets to zero. Without this, a provider that had 2 failures followed by 100 successes would trip on its next single failure. | Low | Critical correctness requirement. Reset happens on every success, not just transitions. |
| Enhanced /health with per-provider circuit state | Operators need visibility into which providers are healthy vs tripped. The current `/health` returns no provider-level information. Kong, Envoy, and KrakenD all expose per-backend health status. | Low | Return provider name, circuit state enum (closed/open/half_open), and consecutive failure count. Top-level status degrades: "ok" -> "degraded" (some open) -> "unhealthy" (all open). |

---

## Differentiators

Features that go beyond the baseline. Not required for correctness but add clear value.

| Feature | Value Proposition | Complexity | Notes |
|---------|-------------------|------------|-------|
| State change logging/tracing events | Emit structured log events on every transition (Closed->Open, Open->Half-Open, Half-Open->Closed, Half-Open->Open). Azure reference explicitly recommends monitoring state changes. Enables alerting via log aggregation. | Low | `tracing::warn!` for Open, `tracing::info!` for recovery. Include provider name, previous state, trigger reason. Practically free to implement. |
| Configurable thresholds in config.toml | Allow global override of `failure_threshold` (default 3) and `recovery_timeout_secs` (default 30) in config. | Low | Add optional `[circuit_breaker]` section. Sensible defaults mean zero-config works. Per-provider overrides are a future option. |
| Circuit state in response headers | `x-arbstr-circuit: provider-alpha=closed,provider-beta=open` lets clients see circuit state without polling /health. | Low | Lightweight addition to the existing header attachment logic. Useful for debugging in development. |
| Open-since and recovery-at timestamps in /health | Show when a circuit opened and when the Half-Open probe will be attempted. Operators can estimate time to recovery without watching logs. | Low | `"open_since": "2026-02-16T10:30:00Z"`, `"recovery_at": "2026-02-16T10:30:30Z"`. Natural extension of the /health response. |
| Half-Open success threshold > 1 | Require N consecutive successes in Half-Open before transitioning to Closed. Prevents flapping when a provider recovers intermittently. Azure pattern specifically recommends this. | Low | Default to 1 for simplicity (matches user spec), make configurable later. |
| Circuit state in /v1/stats response | Include current per-provider circuit state alongside cost/latency aggregates in the existing stats endpoint. | Low | Natural extension of the analytics surface. Minimal new code. |
| Last error details in /health | Track what kind of failures tripped the circuit (timeout, 502, 503, connection refused). Displayed in /health response for diagnostics. | Low | Store last error code and message per provider. Helps operators distinguish between provider-down vs rate-limiting. |

---

## Anti-Features

Features to explicitly NOT build for this milestone.

| Anti-Feature | Why Avoid | What to Do Instead |
|--------------|-----------|-------------------|
| Sliding window failure rate (percentage-based) | Requires tracking request counts over time windows, computing ratios, and tuning window sizes. Resilience4j and SmallRye use this approach because they handle high-throughput distributed services where occasional failures are normal. arbstr is a local proxy with 1-10 providers and low-to-moderate traffic -- a provider failing 3 times in a row is a clear signal, not statistical noise. | Use consecutive failure counter. Simple, deterministic, no tuning required. |
| Active health probing (periodic pings) | Adds a background polling loop, consumes provider API quota (LLM providers charge per request), and requires a health check endpoint that most LLM providers do not expose. Kong supports this but it is designed for services you control. | Use passive detection via real request outcomes. The Half-Open probe uses a real user request, not a synthetic ping. |
| Adaptive/ML-based threshold tuning | Azure docs mention AI-based threshold adjustment as a modern option. Massive overengineering for a local proxy with a handful of providers and straightforward failure modes. | Use static configurable thresholds. Humans can adjust the 2 parameters (failure_threshold, recovery_timeout) if needed. |
| Request queuing/replay when circuit opens | Some patterns buffer requests and replay them when the circuit closes. Adds complexity (queue sizing, ordering, staleness, memory pressure) and LLM requests are often not meaningfully replayable (context changes, user has moved on). | Fail fast with 503 and let the client retry. OpenAI SDKs and other LLM clients already have built-in retry logic. |
| Per-model circuit breakers | A provider might serve multiple models, and theoretically one model could fail while others work. Per-model granularity multiplies state space (providers x models) and complicates routing. | Use per-provider circuit breakers. LLM provider failures are almost always infrastructure-level (the whole endpoint is down), not model-level. If real-world usage shows otherwise, per-model can be added later. |
| Global circuit breaker | Some patterns have a "global" circuit that trips when the entire system is degraded. In arbstr, providers are independent -- a global circuit would prevent routing to healthy providers when only one is down. | Keep circuits strictly per-provider. The "all circuits open" case is handled by returning 503 -- this is the correct global degradation signal. |
| Distributed/shared circuit state | No need for cross-instance state coordination. arbstr is a single-process local proxy, not a distributed fleet. | In-memory state per process. Simple, correct, no external dependencies. |
| Exponential backoff on recovery timeout | Some implementations double the Open-state timeout on each consecutive trip (30s -> 60s -> 120s). Adds complexity and delays recovery detection. | Use fixed timeout. If a provider keeps failing on Half-Open probes, it will cycle through Open/Half-Open states at the fixed interval, which is acceptable behavior for a local proxy. Can add exponential backoff later if needed. |

---

## Feature Dependencies

```
Circuit breaker state machine (core)
  |
  +-> Router integration (filter open circuits from candidates)
  |     |
  |     +-> Outcome recording (feed success/failure back to circuit breaker)
  |     |     |
  |     |     +-> Non-streaming path: after retry+fallback completes
  |     |     |
  |     |     +-> Streaming path: after stream background task completes
  |     |
  |     +-> 503 fail-fast when all providers have open circuits
  |           |
  |           +-> New Error::CircuitOpen variant (maps to 503)
  |
  +-> Enhanced /health endpoint (reads circuit state)
  |
  +-> State change logging (emits tracing events on transitions)

Optional config support
  |
  +-> Circuit breaker state machine (provides threshold overrides)
```

**Critical dependency chain:**
1. Circuit breaker state machine MUST exist before router integration
2. Router integration MUST work before outcome recording (the router decides which providers to try; outcomes flow back)
3. Both MUST work before /health can report meaningful state
4. Config is independent -- can provide defaults and add config support in any order
5. State change logging is a side effect of the state machine -- implement alongside it

---

## Integration Points with Existing Code

### 1. Router (`src/router/selector.rs`)

The `select_candidates()` method currently filters by model and policy constraints, then sorts by cost:

```
Current:  providers -> filter(model) -> filter(policy) -> sort(cost) -> deduplicate
With CB:  providers -> filter(model) -> filter(policy) -> filter(circuit_not_open) -> sort(cost) -> deduplicate
```

**Design decision:** Pass `CircuitBreakerRegistry` reference into `select_candidates()` as a new parameter rather than storing it in the Router. This keeps Router stateless and testable. The handler passes the registry from AppState.

**Half-Open handling:** A provider in Half-Open state should NOT be filtered out. It should be available for selection so that the probe request can flow through. Only Open-state providers are excluded.

**Edge case:** If all providers for a model are in Open state, `select_candidates()` returns an empty list. The handler must distinguish this from the existing "no providers for model" error (400) and return 503 instead.

### 2. Retry Module (`src/proxy/retry.rs`)

The retry module currently records `AttemptRecord` (provider name + status code). Circuit breaker does NOT change the retry module's internal logic -- it operates at a higher level:

- **Before retry:** The router has already filtered out Open providers, so the retry module only sees Closed/Half-Open candidates.
- **After retry:** The handler reads the retry outcome and the attempt history, then feeds results to the circuit breaker:
  - For each failed attempt: call `registry.record_failure(provider_name)`
  - For successful final outcome: call `registry.record_success(provider_name)`

**Important:** The retry module may attempt a request on a provider, fail, and fall back to another provider. Both providers' circuit breakers must be updated: failures recorded for the first, success recorded for the second.

### 3. Streaming Path (`src/proxy/handlers.rs`)

The streaming path does NOT use retry (`execute_request` -> `Router::select` -> `send_to_provider`). Circuit breaker still applies:

- **Selection:** `Router::select` must filter Open providers (same as `select_candidates`)
- **Outcome recording:** The spawned background task already determines success/failure. Add `registry.record_success/failure()` calls in the same code path.
- **503 fast-fail:** If `Router::select` finds no available providers after circuit filtering, return 503 immediately.

### 4. AppState (`src/proxy/server.rs`)

Add `circuit_breakers: Arc<CircuitBreakerRegistry>` to `AppState`. Initialize in `run_server()` from the provider list (one circuit per provider name).

### 5. Health Handler (`src/proxy/handlers.rs`)

Current `/health` handler is a one-liner returning static JSON. Enhanced version:

```json
{
  "status": "ok",
  "service": "arbstr",
  "providers": [
    {
      "name": "provider-alpha",
      "circuit": "closed",
      "consecutive_failures": 0
    },
    {
      "name": "provider-beta",
      "circuit": "open",
      "consecutive_failures": 3,
      "open_since": "2026-02-16T10:30:00Z",
      "recovery_at": "2026-02-16T10:30:30Z"
    }
  ]
}
```

Top-level `status` values:
- `"ok"` -- all circuits closed
- `"degraded"` -- some circuits open, at least one closed
- `"unhealthy"` -- all circuits open (or no providers configured)

### 6. Error Types (`src/error.rs`)

Add `Error::CircuitOpen` variant:

```rust
#[error("All providers for model '{model}' have open circuits")]
CircuitOpen { model: String },
```

Maps to 503 Service Unavailable in `IntoResponse`. Distinct from `Error::NoProviders` (400) which means no provider is configured for the model at all.

---

## MVP Recommendation

**Prioritize (this milestone):**

1. **Circuit breaker state machine** -- The `CircuitBreakerRegistry` with per-provider `CircuitState` enum (Closed { consecutive_failures }, Open { since, consecutive_failures }, HalfOpen { probe_in_flight }), transition methods, and thread-safe access. This is the foundation.

2. **Router integration** -- Filter Open-state providers from `select_candidates()` and `select()`. Preserve Half-Open providers for probe requests.

3. **Outcome recording** -- After every request completes (both streaming and non-streaming paths), record success/failure to the circuit breaker for the provider that handled the request. Also record failures for providers that failed during retry attempts.

4. **503 fail-fast** -- New `Error::CircuitOpen` variant. When router returns empty candidates after circuit filtering (all providers Open), return 503 with OpenAI-compatible error body.

5. **Enhanced /health** -- Per-provider circuit state, consecutive failure count, and top-level status degradation (ok/degraded/unhealthy).

6. **State change logging** -- Emit `tracing::warn!` on Open transitions, `tracing::info!` on Close transitions. Include provider name, previous state, and trigger. Practically free.

**Defer (future milestone):**

- **Configurable thresholds in config.toml** -- Hardcode defaults (3 failures, 30s timeout) for now. Config section can be added without any behavioral changes.
- **Circuit state in response headers** -- Low effort but not in the user's original spec.
- **Half-Open success threshold > 1** -- Start with 1. Add if flapping is observed.
- **Circuit state in /v1/stats** -- Natural extension but not blocking.
- **Last error details in /health** -- Nice for diagnostics, defer to reduce scope.

---

## Complexity Assessment

| Component | Complexity | Risk | Notes |
|-----------|-----------|------|-------|
| CircuitBreakerRegistry data structure | Low | Thread safety must be correct | Enum + counter + timestamp per provider. `RwLock<HashMap<String, CircuitState>>` or `DashMap`. Unit-testable in isolation. |
| State transition logic | Low | Edge cases around concurrent transitions | Deterministic rules: 3 failures -> Open, 30s elapsed -> Half-Open, 1 success in Half-Open -> Closed, 1 failure in Half-Open -> Open. |
| Router filter integration | Low | Must not break existing model/policy filtering | One additional `.filter()` call. Existing tests should still pass with all-Closed circuits. |
| Outcome recording (non-streaming) | Low-Medium | Two integration points in handler | After retry+fallback completes, iterate attempt records for failures, check final outcome for success. |
| Outcome recording (streaming) | Medium | Asynchronous in spawned background task | Must handle the case where circuit state changes between request start and stream completion. The spawned task already has provider name and success/failure determination. |
| Half-Open probe serialization | Medium | Concurrent requests competing for probe slot | Only one request should be the probe. Others should either wait or be routed elsewhere. AtomicBool or state enum flag. |
| Enhanced /health endpoint | Low | None | JSON serialization of circuit state map. Reads only. |
| Error::CircuitOpen variant | Low | Must distinguish from NoProviders | New enum variant + match arm in IntoResponse. |
| Integration tests | Medium | Timing-sensitive for timeout transitions | Use `tokio::test(start_paused = true)` for deterministic time control, matching existing retry test patterns. Mock providers that fail deterministically. |

**Total estimated effort:** Small-to-medium milestone. 2-3 phases. The state machine is conceptually straightforward; most complexity is in correctly wiring outcome recording through both the streaming and non-streaming handler paths.

---

## Sources

- [Circuit Breaker Pattern - Azure Architecture Center](https://learn.microsoft.com/en-us/azure/architecture/patterns/circuit-breaker) -- Definitive reference for 3-state machine, counter approaches, failure counting, recoverability, and design considerations. Updated 2025-02-05. (HIGH confidence)
- [Circuit Breaker - KrakenD API Gateway](https://www.krakend.io/docs/backends/circuit-breaker/) -- Real-world gateway implementation using consecutive errors per backend, with interval, timeout, max_errors configuration (HIGH confidence)
- [Health checks and circuit breakers - Kong Gateway](https://docs.konghq.com/gateway/latest/how-kong-works/health-checks/) -- Active vs passive health checking patterns, per-upstream health status (HIGH confidence)
- [Circuit Breaker vs. Retry Pattern - GeeksforGeeks](https://www.geeksforgeeks.org/system-design/circuit-breaker-vs-retry-pattern/) -- Interaction: retry handles transient failures, circuit breaker handles prolonged outages. Retry should abandon when circuit is open. (MEDIUM confidence)
- [Designing Resilient Systems: Circuit Breakers or Retries? - Grab Engineering](https://engineering.grab.com/designing-resilient-systems-part-1) -- Real-world retry + circuit breaker interaction at scale (MEDIUM confidence)
- [Circuit Breakers - Tyk Documentation](https://tyk.io/docs/planning-for-production/ensure-high-availability/circuit-breakers/) -- Per-endpoint circuit breaking, failure rate approach, 503 on open (HIGH confidence)
- [Protect services with a circuit breaker - ngrok](https://ngrok.com/blog/circuit-breaker-api-gateway) -- Circuit breaker integration at API gateway level with health checks (MEDIUM confidence)
- [503 Service Unavailable for open circuit breaker - GitHub](https://github.com/zalando/problem-spring-web/issues/265) -- Industry consensus on 503 status code for open circuits (MEDIUM confidence)
- [API Circuit Breaker Best Practices - Unkey](https://www.unkey.com/glossary/api-circuit-breaker) -- Setting appropriate thresholds, integrating with health checks (MEDIUM confidence)
- [Outlier detection - Envoy Proxy](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier) -- Per-upstream host ejection based on consecutive errors or success rate (HIGH confidence)
- [CircuitBreaker - Resilience4j](https://resilience4j.readme.io/docs/circuitbreaker) -- Sliding window (count-based and time-based) approach for high-throughput services (HIGH confidence, used for anti-feature rationale)
- [Circuit Breaker Pattern - Box Piper](https://www.boxpiper.com/posts/circuit-breaker-pattern/) -- Comprehensive overview with examples (MEDIUM confidence)
- [Failsafe Circuit Breaker](https://failsafe.dev/circuit-breaker/) -- Consecutive failures vs failure rate approaches (MEDIUM confidence)
