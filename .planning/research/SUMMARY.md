# Project Research Summary

**Project:** arbstr - Per-provider circuit breaker with enhanced /health endpoint
**Domain:** Resilience layer for LLM routing proxy (Rust/axum/tokio)
**Researched:** 2026-02-16
**Confidence:** HIGH

## Executive Summary

arbstr is adding a per-provider circuit breaker to prevent retry storms when providers fail, building on top of existing retry-with-fallback infrastructure. This is a well-established resilience pattern (3-state machine: Closed/Open/HalfOpen) that tracks consecutive failures per provider, trips circuits after a threshold (5 failures), and automatically recovers after a timeout (30s). The circuit breaker filters unavailable providers from routing decisions before retry logic runs, preventing wasted time hitting known-bad providers.

The recommended approach uses DashMap for per-shard concurrent state access (eliminating cross-provider contention), integrates at the handler level (keeping router and retry modules pure), and treats streaming/non-streaming paths equally (recording failures from both). The most critical design decision is placing circuit checks OUTSIDE the retry loop to avoid amplification, while making circuit-open rejections immediately trigger fallback (not consume retry attempts). The enhanced /health endpoint exposes per-provider circuit state with a three-level health model (ok/degraded/unhealthy).

Key risks include retry storm amplification if circuit checks are placed incorrectly, race conditions in failure counting if atomics are used instead of Mutex, and streaming failures being invisible to circuit state if only initial HTTP responses are tracked. These are all preventable through careful integration points and existing codebase patterns (tokio::time::Instant for testability, fire-and-forget spawned tasks for stream completion).

## Key Findings

### Recommended Stack

**Core technologies:**
- **DashMap v6** — Concurrent HashMap with per-shard locking for circuit breaker registry. Eliminates cross-provider contention that would occur with RwLock<HashMap> (any write blocks all reads). 170M+ downloads, stable, minimal dependency weight (only hashbrown transitive, already in tree).
- **tokio::time::Instant** — Monotonic time tracking that responds to tokio::time::pause() for deterministic tests. Critical for testing the 30s Open->HalfOpen timeout without wall-clock waits. Matches existing test patterns (retry.rs uses start_paused = true).
- **std::sync::Mutex** — Wraps CircuitState struct to ensure atomic multi-field updates (state + failure_count + timestamp). NOT tokio::sync::Mutex — circuit operations are pure state machine transitions with no .await points. Lock duration is nanoseconds.

**No changes needed to:** tokio, axum, serde, tracing, reqwest. Existing stack fully supports the circuit breaker.

**Configuration:** Circuit breaker parameters (failure_threshold=5, open_duration=30s, half_open_success_threshold=2) are hardcoded constants initially, NOT in config.toml. Premature configurability forces users to make decisions they lack operational experience to make. Constants are easy to find and change. Promote to config later if users request tuning.

### Expected Features

**Must have (table stakes):**
- 3-state machine (Closed/Open/HalfOpen) — Industry standard from Azure Architecture, implemented by all production proxies (Envoy, Kong, KrakenD). Anything less is just a blocklist.
- Consecutive failure threshold to trip Open — Simple, deterministic, correct for a local proxy with low traffic volume. Failure-rate sliding windows are overkill (see Anti-Features).
- Timeout-based transition to HalfOpen — Without recovery mechanism, tripped providers can never recover until process restart.
- Single probe request in HalfOpen — Allow exactly one request through when timeout expires. Success->Closed, failure->Open.
- Fail-fast 503 when all circuits open — Standard response code across all proxies. Distinct from 400 "no providers configured" error.
- Skip open-circuit providers in routing — Core value proposition. Filter in handler before retry starts.
- Success resets consecutive failure counter — Critical correctness requirement. Without this, accumulated non-consecutive failures trip the circuit.
- Enhanced /health with per-provider circuit state — Operators need visibility. Kong, Envoy, KrakenD all expose per-backend health. Three-level status: ok/degraded/unhealthy.

**Should have (competitive differentiators):**
- State change logging — Emit tracing::warn! on Open, tracing::info! on recovery. Enables alerting via log aggregation. Practically free to implement.
- Configurable thresholds in config.toml — Allow global override of failure_threshold and recovery_timeout_secs. Low effort, sensible defaults mean zero-config works.
- Circuit state in response headers — x-arbstr-circuit-state header shows which providers were filtered. Useful for debugging.
- Open-since and recovery-at timestamps in /health — Operators can estimate time to recovery without log watching.

**Defer (anti-features for v1):**
- Sliding window failure rate (percentage-based) — Requires time window tracking, ratio computation, tuning. arbstr is a local proxy with 1-10 providers and low-moderate traffic. Consecutive failure counting is sufficient.
- Active health probing (periodic pings) — Adds background polling, consumes provider API quota, requires health endpoint most LLM providers don't expose. Use passive detection via real request outcomes.
- Adaptive/ML-based threshold tuning — Massive overengineering for a local proxy.
- Request queuing/replay — LLM requests are not meaningfully replayable. Fail fast with 503, let client retry.
- Per-model circuit breakers — Multiplies state space (providers x models). LLM provider failures are almost always infrastructure-level (entire endpoint down), not model-specific.
- Global circuit breaker — Would prevent routing to healthy providers when only one is down. Keep circuits strictly per-provider.
- Distributed/shared circuit state — arbstr is single-process local proxy, not a distributed fleet. In-memory state is correct.

### Architecture Approach

The circuit breaker lives in a new `src/proxy/circuit_breaker.rs` module at the proxy layer, NOT in the router (which stays pure: model/cost/policy selection only) or storage (circuit state is ephemeral, in-memory). It integrates with existing code at the handler level, filtering provider candidates BEFORE retry logic runs and recording outcomes AFTER requests complete.

**Major components:**
1. **CircuitBreakerRegistry** — DashMap<String, ProviderCircuitBreaker> shared via Arc in AppState. Provides filter_available(), record_success(), record_failure(), provider_states() methods. Thread-safe, cheaply cloneable.
2. **ProviderCircuitBreaker** — Per-provider state machine with CircuitState enum (Closed/Open/HalfOpen), failure_count, opened_at timestamp, half_open_successes counter. Wrapped in Mutex for atomic multi-field updates. Plain fields (not atomics) because DashMap provides per-shard locking.
3. **Integration hooks** — Handler filters candidates through circuit breaker between router.select_candidates() and retry_with_fallback(). After retry outcome, records success/failure for each attempted provider. Streaming path records outcomes in spawned background task (circuit registry is Send + Sync).
4. **Enhanced /health handler** — Accepts State<AppState>, queries circuit_breakers.provider_states(), adds database ping, returns structured JSON with per-provider circuit state and overall status (ok/degraded/unhealthy).

**Request flow:**
```
1. chat_completions handler receives request
2. router.select_candidates() returns Vec<SelectedProvider> sorted cheapest-first
3. circuit_breakers.filter_available(candidates) removes Open-state providers  <-- NEW
4. If empty after filter, return 503 immediately (all circuits open)           <-- NEW
5. retry_with_fallback(filtered_candidates, ...)
6. send_to_provider() makes HTTP call
7. Record outcome: circuit_breakers.record_success/failure(provider_name)      <-- NEW
```

**Lazy state transitions:** Open->HalfOpen transition is checked lazily when is_available() is called, not via background timer. No background tasks to manage. Transition happens exactly when needed (at request time). Simpler to test with tokio::time::pause().

### Critical Pitfalls

1. **Retry storm amplification if circuit checks are inside retry loop** — If circuit breaker checks happen PER-ATTEMPT instead of PRE-ROUTING, a downed provider consumes all 3 retry attempts (plus 1s+2s backoff = 3s minimum) before circuit trips. With 10 concurrent requests, that's 30 failed attempts hammering a downed provider. Prevention: Filter candidates BEFORE entering retry_with_fallback. Circuit-open rejections must trigger immediate fallback, not consume retry attempts. Modify is_retryable() to treat circuit-open as "skip this provider, try next" without backoff.

2. **Race conditions in consecutive failure counting with atomics** — Using AtomicU32 for failure_count and AtomicU8 for state creates TOCTOU races. Two threads can both see count==3 and attempt transition. Worse: success can reset counter while another thread is incrementing, breaking "consecutive" semantics. Prevention: Use Mutex<CircuitState> wrapping ALL mutable fields (state, failure_count, timestamps). Lock duration is nanoseconds (pure state machine transitions, no I/O). Matches existing codebase pattern (Arc<Mutex<Vec<AttemptRecord>>> in retry module).

3. **Streaming failures invisible to circuit breaker** — Streaming path returns 200 OK immediately (provider accepted connection). Real failures (stream interruption, timeout) are discovered in spawned background task AFTER response is returned. If circuit breaker only records initial HTTP status, streaming failures never trip circuits. Prevention: Record outcomes in spawned stream completion task. Circuit registry must be Arc (shared across threads) and methods must be Send + Sync. Only record success when done_received==true. Mid-stream failures count equally toward circuit threshold.

4. **Circuit breaker hides providers from router, creating misleading "no providers" 400 errors** — If all providers for a model have open circuits, filtering returns empty candidate list. Existing error handling treats empty list as Error::NoProviders (400 Bad Request). Clients see "No providers available" which implies misconfiguration, not transient failure. Prevention: New error variant Error::AllCircuitOpen mapping to 503 with Retry-After header. Include x-arbstr-circuit-state response header showing which providers were skipped.

5. **Half-open probe race — multiple requests enter HalfOpen simultaneously** — When circuit transitions Open->HalfOpen (after 30s timeout), multiple concurrent requests check state simultaneously, all see HalfOpen, and all send probe requests. Recovering provider gets burst instead of single probe. Worse: if some succeed and some fail, last transition wins, causing circuit flapping. Prevention: Use single-permit model with probe_in_flight bool flag. Only first request that atomically claims the permit (inside Mutex) sends probe. All other concurrent requests during HalfOpen rejected as if circuit still Open.

## Implications for Roadmap

Based on research, suggested phase structure:

### Phase 1: Circuit Breaker Core (foundation)
**Rationale:** State machine must exist and be correct before any integration. This is independently testable with no HTTP server, no database, no routes. Pure unit tests with deterministic time control (tokio::test start_paused).
**Delivers:** src/proxy/circuit_breaker.rs with CircuitState enum, CircuitBreakerConfig, ProviderCircuitBreaker state machine (is_available, record_success, record_failure methods), CircuitBreakerRegistry with DashMap, ProviderHealthInfo struct for /health.
**Addresses:** 3-state machine, consecutive failure threshold, timeout-based transition, half-open single-probe (table stakes from FEATURES.md).
**Avoids:** Pitfall 2 (uses Mutex not atomics), Pitfall 5 (single-permit half-open model), Pitfall 9 (uses tokio::time::Instant for testability).
**Tests:** Pure unit tests for all state transitions, failure threshold boundary behavior, success resets counter, half-open probe logic. Use start_paused = true with tokio::time::advance() for deterministic 30s timeout tests.

### Phase 2: Handler Integration (Non-Streaming)
**Rationale:** Non-streaming path handles all retried requests (majority of traffic). This phase delivers the core circuit protection value. Streaming can be added separately because it uses a completely different code path (execute_request vs retry_with_fallback).
**Delivers:** Modified src/proxy/handlers.rs chat_completions handler to filter candidates through circuit_breakers.filter_available() BEFORE retry_with_fallback(), return 503 if all circuits open, record success/failure AFTER retry outcome. Modified src/proxy/server.rs to add circuit_breakers to AppState, initialize from config provider names.
**Addresses:** Skip open-circuit providers in routing, fail-fast 503 when all open, success resets counter (table stakes).
**Avoids:** Pitfall 1 (filters PRE-ROUTING not inside retry loop), Pitfall 4 (new error variant for all-circuit-open distinct from NoProviders), Pitfall 6 (only count 5xx as circuit failures, reuse is_retryable logic), Pitfall 10 (mandate record_success on every Ok path).
**Tests:** Integration tests with mock providers. Trigger 5 consecutive failures, verify circuit opens. Verify next request skips open provider and uses fallback. Verify request with all providers open returns 503 immediately.

### Phase 3: Enhanced /health Endpoint
**Rationale:** Observability surface for external monitoring. Can be implemented in parallel with Phase 2 (no dependency) or immediately after. Low risk, purely additive (new response format, no behavior change).
**Delivers:** Modified src/proxy/handlers.rs health handler to accept State<AppState>, query circuit_breakers.provider_states(), add database ping, return structured JSON with per-provider circuit state and overall status.
**Addresses:** Enhanced /health with per-provider circuit state (table stakes).
**Avoids:** Pitfall 8 (three-level health model: ok when all closed, degraded when some open, unhealthy when all open).
**Tests:** Integration tests for /health response structure. Verify reflects circuit state changes. Verify top-level status degrades appropriately.

### Phase 4: Streaming Path Integration
**Rationale:** Streaming path is separate from retry logic (execute_request directly calls router.select, no retry). Lower priority because streaming already has no retry protection, so circuit breaker is additive value but not critical for correctness. Can be deferred if needed.
**Delivers:** Modified src/proxy/handlers.rs execute_request to check circuit breaker before using selected provider, fall back to select_candidates + filter if primary is circuit-broken. Record outcome in spawned background task after stream completes.
**Addresses:** Equal treatment of streaming and non-streaming failures for circuit health.
**Avoids:** Pitfall 3 (streaming failures recorded via spawned task callback).
**Tests:** Integration tests for streaming path with circuit-broken providers. Verify mid-stream failures count toward threshold.

### Phase 5: Polish and Observability
**Rationale:** Nice-to-haves that improve debugging and monitoring but are not blocking. Can be done incrementally.
**Delivers:** State change logging (tracing::info! on transitions), x-arbstr-circuit-state response header, open-since and recovery-at timestamps in /health response, Error::CircuitOpen variant for clearer error messages.
**Addresses:** Differentiators from FEATURES.md (state change logging, circuit state in response headers).
**Avoids:** N/A (polish phase).
**Tests:** Verify logs emit on state transitions with correct structured fields. Verify response headers appear when filtering occurs.

### Phase Ordering Rationale

- Phase 1 first because state machine is foundation — all integration depends on it. Independently testable with no integration risk.
- Phase 2 next because non-streaming path is highest value (handles all retried requests). This delivers core circuit protection.
- Phase 3 can run concurrently with Phase 2 (no dependency) or immediately after. Purely additive, low risk.
- Phase 4 deferred because streaming path is lower priority (already no retry, so circuit breaker is additive value but not critical).
- Phase 5 last because polish is not blocking. Can be done incrementally or deferred to v2.

Phases 1-3 deliver complete circuit breaker protection for non-streaming requests plus full observability. This is the MVP. Phase 4 extends to streaming. Phase 5 is polish.

### Research Flags

Phases with standard patterns (skip research-phase):
- **Phase 1:** Circuit breaker state machine is well-documented pattern (Azure Architecture Center, Martin Fowler, Resilience4j). Implementation is straightforward Rust state machine.
- **Phase 2:** Integration points identified through direct codebase analysis (retry.rs, handlers.rs, selector.rs all read and understood).
- **Phase 3:** /health endpoint enhancement is trivial JSON serialization of circuit state snapshot.
- **Phase 4:** Streaming path integration follows same pattern as Phase 2, just different code path.
- **Phase 5:** Logging and headers are standard axum/tracing patterns.

**No phases need /gsd:research-phase.** All patterns are established and integration points are known from codebase analysis.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | DashMap verified as standard concurrent map (170M+ downloads). tokio::time::Instant verified as correct for deterministic tests (existing retry.rs tests use start_paused). Mutex vs atomics decision based on direct analysis of race conditions. |
| Features | HIGH | Table stakes features verified against Azure Architecture Center (canonical circuit breaker reference), Kong/Envoy/KrakenD docs (production proxy implementations). Anti-features justified based on arbstr's context (local proxy, low traffic volume, no distributed coordination). |
| Architecture | HIGH | Integration points identified through direct codebase inspection (retry.rs, handlers.rs, stream.rs, selector.rs, server.rs all read). Handler-level integration (not router or middleware) matches existing patterns. Lazy state transitions match existing time-based logic. |
| Pitfalls | HIGH | Retry amplification math is deterministic (MAX_RETRIES=2 means 3 attempts). Race conditions verified against Rust Atomics and Locks reference. Streaming invisibility verified from actual fire-and-forget spawned task pattern in handlers.rs. Half-open race documented in Azure/Resilience4j sources. |

**Overall confidence:** HIGH

### Gaps to Address

No significant gaps. Research is complete and actionable.

Minor areas to validate during implementation:
- **Exact circuit-open error handling in retry module:** The retry_with_fallback function may need a small modification to distinguish "circuit open, skip to fallback" from "request failed, retry with backoff." The is_retryable() function provides a template, but circuit-open may need a third outcome ("skip" vs "retry" vs "abort"). This is a small implementation detail, not a research gap.
- **Mock provider 503 behavior in tests:** Integration tests will need mock providers that deterministically return 503 to trip circuits. The existing mock mode may need minor extension. Not a blocker — mock server can be inline in test.

## Sources

### Primary (HIGH confidence)
- Local codebase analysis: Cargo.toml, src/proxy/server.rs (AppState), src/proxy/handlers.rs (health handler, send_to_provider, streaming path), src/proxy/retry.rs (retry pattern, start_paused tests, is_retryable), src/router/selector.rs (select_candidates), src/proxy/stream.rs (spawned task pattern) — all read and verified
- [Circuit Breaker Pattern - Azure Architecture Center](https://learn.microsoft.com/en-us/azure/architecture/patterns/circuit-breaker) — Definitive reference for 3-state machine, counter approaches, concurrency considerations, half-open permit model
- [Martin Fowler - Circuit Breaker](https://martinfowler.com/bliki/CircuitBreaker.html) — Canonical pattern description
- [DashMap documentation](https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html) — Per-shard locking confirmed
- [DashMap on crates.io](https://crates.io/crates/dashmap) — 170M+ downloads, version 6.x stable
- [tokio::time::Instant docs](https://docs.rs/tokio/latest/tokio/time/struct.Instant.html) — Virtual time control for testing confirmed

### Secondary (MEDIUM confidence)
- [Retry Storm Antipattern - Azure Architecture Center](https://learn.microsoft.com/en-us/azure/architecture/antipatterns/retry-storm/) — Retry amplification, circuit breaker as mitigation
- [Resilience4j CircuitBreaker Documentation](https://resilience4j.readme.io/docs/circuitbreaker) — Atomic state management, sliding window (anti-pattern for arbstr), half-open permit model
- [Circuit Breaker - KrakenD API Gateway](https://www.krakend.io/docs/backends/circuit-breaker/) — Real-world gateway using consecutive errors per backend
- [Health checks and circuit breakers - Kong Gateway](https://docs.konghq.com/gateway/latest/how-kong-works/health-checks/) — Active vs passive health checking, per-upstream status
- [Circuit Breakers - Tyk Documentation](https://tyk.io/docs/planning-for-production/ensure-high-availability/circuit-breakers/) — Per-endpoint circuit breaking, 503 on open
- [Outlier detection - Envoy Proxy](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/outlier) — Per-upstream host ejection based on consecutive errors
- [Resilience Design Patterns - Codecentric](https://www.codecentric.de/en/knowledge-hub/blog/resilience-design-patterns-retry-fallback-timeout-circuit-breaker) — Pattern composition guidance
- [Linkerd Circuit Breaking](https://linkerd.io/2-edge/reference/circuit-breaking/) — Per-endpoint circuit breaking in proxy context

### Tertiary (LOW confidence)
- [failsafe crate docs](https://docs.rs/failsafe/latest/failsafe/) — Evaluated but architectural mismatch (call-wrapping API conflicts with retry architecture)
- [tower-circuitbreaker docs](https://docs.rs/tower-circuitbreaker/latest/tower_circuitbreaker/) — Evaluated but wrong abstraction (Tower Service middleware operates on individual requests, not routing decisions)

---
*Research completed: 2026-02-16*
*Ready for roadmap: yes*
