# Domain Pitfalls: Per-Provider Circuit Breaker with Existing Retry/Fallback

**Domain:** Adding per-provider circuit breaker to an existing Rust async proxy with retry and exponential backoff
**Researched:** 2026-02-16
**Scope:** Circuit breaker state machine concurrency, interaction with existing retry_with_fallback logic, streaming vs non-streaming asymmetry, half-open probe race conditions, /health endpoint accuracy, failure counter semantics
**Confidence:** HIGH (based on direct codebase analysis of retry.rs/handlers.rs/stream.rs/selector.rs, Microsoft Azure architecture docs, Martin Fowler circuit breaker pattern, Rust atomics/concurrency references)

---

## Critical Pitfalls

Mistakes that cause retry amplification, silent provider starvation, or data races in the circuit breaker state machine. These would require significant rework if discovered after implementation.

---

### Pitfall 1: Retry Storm Amplification -- Circuit Breaker Inside vs Outside Retry Loop

**What goes wrong:** The existing `retry_with_fallback` in `src/proxy/retry.rs` retries up to 3 times on the primary provider before falling back. If the circuit breaker check is placed INSIDE the `send_request` closure (per-attempt), a provider that is genuinely down will consume all 3 retry attempts plus backoff delays (1s + 2s = 3s minimum) before the circuit trips. During those 3 attempts, the failure counter increments but the circuit remains closed. Meanwhile, every concurrent request hitting the same provider also burns through its full retry budget. With 10 concurrent requests, that is 30 failed attempts hammering a downed provider before anyone sees a circuit-open rejection.

The inverse mistake is also dangerous: if the circuit breaker wraps the ENTIRE retry_with_fallback call (outside the loop), then the circuit tracks "did the full retry chain succeed" rather than "did this specific provider succeed." A single provider's failures get masked by successful fallback, and the circuit never opens for the bad provider.

**Why it happens:** The existing code separates provider selection (`select_candidates`) from the retry loop. The retry loop calls `send_to_provider` per candidate. Developers naturally ask "where does the circuit check go?" and either choice has a trap.

**Consequences:**
- Circuit inside retry: 3x failure amplification before trip, defeating the circuit breaker's purpose of fail-fast
- Circuit outside retry: per-provider failures never trip because the fallback masks them; downed providers keep getting retried forever
- Both create retry storms when multiple requests arrive concurrently during provider degradation

**Prevention:**
1. The circuit breaker check MUST be at the per-provider, per-attempt level -- inside `send_to_provider`, not wrapping `retry_with_fallback`. Each call to a provider should: (a) check circuit state first, (b) if open, return a specific "circuit open" error type immediately, (c) if closed/half-open, proceed with the request.
2. The "circuit open" error MUST be treated as non-retryable by the existing `is_retryable()` function. Currently `is_retryable` only returns true for 500/502/503/504. A circuit-open rejection should NOT be retryable on the same provider.
3. However, a circuit-open rejection MUST trigger immediate fallback to the next candidate -- not a hard failure. This means adding a new error variant or status code that `retry_with_fallback` recognizes as "skip this provider, try next" without consuming a retry attempt.
4. The retry loop must be modified so circuit-open errors do not increment the retry counter or trigger backoff delays. The current loop structure (lines 137-166 of retry.rs) does `attempt in 0..=MAX_RETRIES` with backoff, but a circuit-open rejection should bypass this entirely and jump straight to fallback.

**Specific code impact on `retry_with_fallback`:**
```
Current: primary fails -> record attempt -> backoff -> retry primary -> ... -> try fallback
Needed:  primary circuit-open -> skip to fallback immediately (no attempt recorded, no backoff)
         primary circuit-closed but request fails -> record attempt -> record_failure in circuit -> backoff -> retry
```

**Phase to address:** Must be the FIRST design decision in the circuit breaker implementation phase. The interaction between circuit breaker and retry is the architectural spine -- every subsequent decision depends on getting this layering right.

**Confidence:** HIGH -- the retry_with_fallback code is explicitly structured around `is_retryable()` (line 148 of retry.rs) and the fallback path (lines 169-186). The amplification math is deterministic: MAX_RETRIES=2 means 3 attempts per provider per request.

---

### Pitfall 2: Consecutive Failure Counter Race Between Concurrent Requests

**What goes wrong:** The circuit breaker spec calls for "3 consecutive failures to open." With an `AtomicU32` failure counter and an `AtomicU8` state, a naive implementation does:
```rust
let count = failures.fetch_add(1, Ordering::SeqCst) + 1;
if count >= THRESHOLD {
    state.store(OPEN, Ordering::SeqCst);
}
```

This has a TOCTOU (time-of-check-to-time-of-use) race: two concurrent requests can both see `count == 3` (both incremented from 2 to 3), and both attempt the state transition. While the double-transition to Open is idempotent and harmless for opening, the real danger is in the RESET path. When a success occurs while the counter is being incremented by another thread:

```
Thread A: request succeeds -> resets counter to 0
Thread B: request fails -> fetch_add(1) reads 0, sets to 1
Thread A: (reset already happened)
Thread C: request fails -> fetch_add(1) reads 1, sets to 2
Thread D: request fails -> fetch_add(1) reads 2, sets to 3 -> trips circuit
```

This appears correct, but consider interleaving where success and failure happen simultaneously:

```
Thread A: request starts (circuit closed)
Thread B: request starts (circuit closed)
Thread A: request fails -> count becomes 1
Thread B: request succeeds -> reset count to 0
Thread A: was this "consecutive"? The counter says 1 but there was a success in between.
```

The counter correctly reset to 0 in this case. But with pure atomics (no lock), there is no way to atomically check "is the circuit still closed AND increment the failure count." A request can read state=Closed, then another thread opens the circuit, and the first thread increments the failure counter of an already-open circuit (the counter value is now meaningless because it is tracking failures in the Open state).

**Why it happens:** Developers reach for atomics because they are faster than Mutex/RwLock and the circuit state seems like a simple integer. But the circuit breaker is a multi-field state machine: (state, failure_count, last_failure_time, last_success_time) -- you cannot atomically update all of them.

**Consequences:**
- Failure counter drifts: counts failures that happened during Open state (when they should not count because requests are being rejected)
- Counter never reaches threshold in high-concurrency because interleaved successes keep resetting it (circuit never opens despite persistent failures)
- Counter reaches threshold despite successful requests (circuit opens spuriously)
- Half-open probe results get corrupted by racing transitions (see Pitfall 5)

**Prevention:**
1. Use a `Mutex<CircuitState>` struct that holds ALL mutable state together (state enum, failure_count, opened_at timestamp, etc.). The Mutex ensures atomic multi-field updates.
2. The performance concern is overblown: the critical section is tiny (compare an integer, maybe update two fields). With `std::sync::Mutex` (not `tokio::sync::Mutex`), the lock is never held across an await point. Contention is negligible because the lock duration is nanoseconds.
3. Do NOT use `tokio::sync::Mutex` -- the circuit check/update should be synchronous (no .await inside the lock). `std::sync::Mutex` is correct here and avoids the "holding Mutex across await" footgun.
4. The `Arc<Mutex<CircuitState>>` pattern matches the existing code's use of `Arc<Mutex<Vec<AttemptRecord>>>` in the retry module (line 9 of retry.rs, line 298 of handlers.rs), so it is consistent with codebase conventions.

**Alternative -- CAS loop with packed atomic:**
Pack `(state: u8, failure_count: u8, generation: u16)` into a single `AtomicU32` and use `compare_exchange` for transitions. This is lock-free but complex to implement correctly, hard to extend (no room for timestamps), and the performance benefit over Mutex is irrelevant at arbstr's concurrency level (proxy handling maybe hundreds of concurrent requests, not millions).

**Phase to address:** Circuit breaker state struct design. Must be decided before any transition logic is written.

**Confidence:** HIGH -- the race conditions are well-documented in concurrent circuit breaker literature (Resilience4j uses a Ring Bit Buffer with synchronized transitions; Microsoft Azure docs explicitly warn that "a large number of concurrent instances of an application can access the same circuit breaker" and "the implementation shouldn't block concurrent requests or add excessive overhead").

---

### Pitfall 3: Streaming Requests Silently Bypass Circuit Breaker Recording

**What goes wrong:** The existing code has completely separate paths for streaming and non-streaming requests:

- **Non-streaming** (handlers.rs lines 234-474): goes through `retry_with_fallback` -> `send_to_provider` -> gets response -> knows success/failure immediately
- **Streaming** (handlers.rs lines 154-233): calls `execute_request` directly (NO retry), gets back a 200 OK with a streaming body, spawns a background task that discovers success/failure AFTER the stream completes

The circuit breaker records failures to track provider health. For streaming requests, the initial HTTP response is 200 OK (the provider accepted the connection and started streaming). The REAL failure -- stream interruption, incomplete response, timeout -- is only discovered inside the `tokio::spawn` background task (handlers.rs lines 712-808) when `stream_result.done_received` is false.

If the circuit breaker only records the initial HTTP status (200 OK for streaming), streaming failures are invisible to it. A provider that consistently drops streaming connections after 5 seconds will never trip the circuit because every initial response was "successful."

**Why it happens:** The response for streaming is returned to the client immediately (line 821-828 of handlers.rs). The circuit breaker would naturally be checked/updated in the request-response flow, before the background task runs. The background task's completion is fire-and-forget.

**Consequences:**
- Providers that fail during streaming never get their circuits opened
- Non-streaming requests to the same provider trip the circuit correctly, creating inconsistent behavior
- Users see streaming requests consistently failing mid-stream with no circuit protection
- The /health endpoint shows a provider as "healthy" even though every streaming request to it fails

**Prevention:**
1. For the INITIAL connection phase (before streaming body starts), record success/failure normally. A 502/503/500 before streaming starts should count as a circuit breaker failure.
2. For stream-phase failures (connection drops, incomplete responses), use the existing `spawn_stream_completion_update` callback path. After the background task determines the stream was incomplete (`done_received == false`), it should also record a circuit breaker failure.
3. This means the circuit breaker needs a `record_failure(provider)` method that can be called from a spawned task -- it must be `Send + Sync + 'static` (which `Arc<Mutex<CircuitState>>` satisfies).
4. Decide on policy: should a single mid-stream failure count the same as a connection failure? Recommendation: YES, count it equally. A provider that drops streams is equally unhealthy as one that refuses connections. The "consecutive" counter should treat both failure modes identically.
5. However, do NOT record stream success until the stream actually completes successfully. The initial 200 OK is not a success for circuit breaker purposes when streaming -- only `done_received == true` counts.

**Code impact:**
```
// In the spawned task (handlers.rs ~line 795):
if !success {
    // Record circuit breaker failure for this provider
    circuit_breaker.record_failure(&provider_name);
}
// On success:
if success {
    circuit_breaker.record_success(&provider_name);
}
```

**Phase to address:** Must be addressed in the same phase as the circuit breaker implementation. Deferring streaming integration creates a false sense of protection.

**Confidence:** HIGH -- the streaming path is clearly separate in handlers.rs (line 154 `if is_streaming` branches to `execute_request` which calls `router.select` not `select_candidates`, bypasses retry entirely). The fire-and-forget pattern is visible at line 712 (`tokio::spawn`).

---

### Pitfall 4: Circuit Breaker Hides Provider From Router, Creating "No Providers" Errors

**What goes wrong:** The router's `select_candidates()` returns providers sorted by cost. If the circuit breaker is applied as a filter in the router (removing open-circuit providers from the candidate list), then when ALL providers for a model have open circuits, the candidate list is empty and the router returns `Error::NoProviders`. The client sees a 400 Bad Request ("No providers available for model 'gpt-4o'") which is misleading -- providers ARE configured, they are just all circuit-broken.

Worse, the existing error handling in handlers.rs (lines 243-287) treats `Error::NoProviders` as a non-retryable pre-routing error. The request fails immediately with no attempt at fallback or waiting.

**Why it happens:** The natural place to integrate the circuit breaker is in the routing layer -- "don't select providers with open circuits." This is correct for cost optimization (don't waste time on downed providers) but wrong for error reporting and the "all providers down" edge case.

**Consequences:**
- Clients get 400 (Bad Request) instead of 503 (Service Unavailable) when all providers are circuit-broken
- The error message says "No providers available" which implies misconfiguration, not transient failure
- No Retry-After header to tell clients when to try again (the circuit timeout is known but not communicated)
- Monitoring/alerting systems designed to watch for 503s never fire; they see 400s instead

**Prevention:**
1. Separate circuit filtering from the router. The router should return ALL matching providers (circuit-open or not). The circuit breaker should be checked at the point of use (in `send_to_provider` or a wrapper).
2. When all providers are circuit-open, return a new error variant `Error::AllProvidersCircuitOpen` that maps to HTTP 503 with an appropriate message and a `Retry-After` header set to the soonest circuit half-open time.
3. Add an `x-arbstr-circuit-state` response header showing which providers were skipped due to open circuits, for observability.
4. In the candidate selection, sort circuit-open providers LAST (not removed). This way the retry loop tries closed providers first, then half-open, then as a last resort, open-circuit providers (which will fail fast with the circuit-open error and trigger the 503).

**Error variant:**
```rust
// In error.rs:
#[error("All providers have open circuits for model '{model}'")]
AllCircuitOpen { model: String, retry_after_secs: u64 },
```

**Phase to address:** Router integration phase. Must be decided before the circuit breaker is wired into the request path.

**Confidence:** HIGH -- the `NoProviders` error path is clearly defined (error.rs line 16, handlers.rs line 247) and maps to 400. The disconnect between "no providers configured" and "all providers broken" is a well-known circuit breaker integration problem documented in the Azure Architecture Center ("Be careful when you use a single circuit breaker for one type of resource if there might be multiple underlying independent providers").

---

### Pitfall 5: Half-Open Probe Race -- Multiple Requests Enter Half-Open Simultaneously

**What goes wrong:** When the circuit transitions from Open to Half-Open (after 30s timeout), the intent is to allow ONE probe request through to test the provider. But in a concurrent system, multiple requests check the circuit state simultaneously, all see "Half-Open" (or all see "Open with expired timer"), and all attempt to send their request to the recovering provider.

The state machine spec says: in Half-Open, if the probe succeeds, close the circuit; if it fails, re-open. With N requests all probing simultaneously:
- If the provider has recovered: all N succeed, which is fine but the recovering provider gets hit with a burst
- If the provider is still down: all N fail, which is fine functionally but wasteful
- If the provider is partially recovered: some succeed, some fail. The last transition wins. With bad timing, a success transitions to Closed, then immediately a slow failure (from a request that started earlier) transitions to Open, causing circuit flapping

**Why it happens:** Checking `if state == HalfOpen && now > opened_at + timeout` is not an atomic "check and claim" operation. Even with a Mutex, the window between releasing the Mutex (after reading state) and acquiring it again (after the request completes) allows other threads to also see Half-Open and send their requests.

**Consequences:**
- Recovering providers get hammered with burst traffic instead of gradual probe
- Circuit state oscillates between Closed and Open ("flapping"), causing inconsistent routing
- Metrics become unreliable: the circuit reports frequent state transitions that do not reflect actual provider health

**Prevention:**
1. Use a "permit" model for half-open probes: when transitioning from Open to Half-Open, atomically set a flag `probe_in_flight: bool`. Only the first request that successfully claims the permit (sets it from false to true inside the Mutex) sends the probe. All other concurrent requests during Half-Open should be rejected as if the circuit is still Open.
2. Inside the Mutex, the transition logic should be:
   ```
   if state == Open && now >= opened_at + timeout {
       state = HalfOpen;
       probe_in_flight = true;  // claimed by this request
   } else if state == HalfOpen && probe_in_flight {
       return Err(CircuitOpen);  // another request is already probing
   }
   ```
3. When the probe request completes (success or failure), acquire the Mutex and transition based on the result. Reset `probe_in_flight = false`.
4. Do NOT let the probe be a streaming request if avoidable. Streaming probes take longer to determine success/failure (must wait for stream completion), extending the Half-Open window and increasing the chance of racing. Prefer non-streaming health checks or ensure the probe timeout is shorter than the stream timeout.

**Phase to address:** Core circuit breaker state machine implementation. This is not an optimization -- it is correctness.

**Confidence:** HIGH -- the Microsoft Azure Architecture Center explicitly addresses this: "A limited number of requests from the application are allowed to pass through and invoke the operation" and "the Half-Open state helps prevent a recovering service from suddenly being flooded with requests." The single-permit model is the standard solution (Resilience4j uses `permittedNumberOfCallsInHalfOpenState`).

---

## Moderate Pitfalls

Issues that cause incorrect behavior or poor observability but do not cause data races or cascading failures.

---

### Pitfall 6: Non-Retryable Errors (4xx) Incorrectly Counted as Circuit Breaker Failures

**What goes wrong:** The circuit breaker failure counter should only count failures that indicate the PROVIDER is unhealthy (network errors, 500, 502, 503, 504). A 400 Bad Request or 401 Unauthorized means the REQUEST was wrong, not that the provider is broken. If 4xx errors count toward the circuit breaker threshold, a client sending malformed requests can trip the circuit for a healthy provider.

The existing `is_retryable()` in retry.rs (line 68) already distinguishes retryable (5xx) from non-retryable (everything else). The circuit breaker must make the same distinction but developers might not connect the two.

**Why it happens:** The `send_to_provider` function in handlers.rs (lines 483-565) returns `Err(RequestError)` for both 4xx and 5xx responses. The error struct has a `status_code` field that can be inspected, but the circuit breaker needs to be told which errors count.

**Consequences:**
- A client sending 401s (expired API key) trips the circuit for all users of that provider
- A burst of 429 (rate limit) responses trips the circuit even though the provider is healthy (just throttled)
- Healthy providers become unavailable due to client-side issues

**Prevention:**
1. Reuse the same `is_retryable()` logic from retry.rs for circuit breaker failure counting. Only HTTP 500, 502, 503, 504 and connection-level errors (DNS failure, TCP timeout, TLS failure) should count.
2. HTTP 429 is a special case: the provider IS responding but is rate-limiting. This should NOT trip the circuit. However, you might want a separate "throttled" signal for routing purposes (deprioritize throttled providers). This is a future enhancement, not a circuit breaker concern.
3. Connection-level errors (reqwest returns an error before any HTTP status, visible at handlers.rs line 520-531 where the error gets status_code 502) MUST count as failures. These indicate the provider is unreachable.
4. Create a `should_count_as_circuit_failure(status_code: u16, is_connection_error: bool) -> bool` function and place it next to `is_retryable()` in retry.rs for discoverability.

**Phase to address:** Circuit breaker failure recording implementation. Should be in the same PR as the core state machine.

**Confidence:** HIGH -- the distinction between provider-side and client-side errors is well-established. The existing `is_retryable()` function provides a clear model to follow.

---

### Pitfall 7: Circuit Breaker State Not Persisted Across Restarts

**What goes wrong:** If the circuit breaker state is purely in-memory (which it should be for v1), restarting arbstr resets all circuits to Closed. If a provider is genuinely down and arbstr restarts, requests will immediately hit the downed provider again, experiencing 3 failures before re-tripping the circuit. This is usually acceptable for a local proxy, but becomes a problem if arbstr is being restarted frequently (crash loops, deployments) during a provider outage.

**Why it happens:** In-memory state is the simplest correct implementation. Persistence adds complexity (SQLite writes on every state change, schema migration, stale state on startup).

**Consequences:**
- Each restart resets the circuit, causing a brief burst of failures to downed providers
- During rapid restarts (crash loop), the circuit never stays open long enough to protect the system
- If arbstr is deployed behind a load balancer with multiple instances, each instance has independent circuit state

**Prevention:**
1. For v1, accept in-memory state. Document the behavior. The 3-failure threshold means recovery is fast (3 requests at worst before the circuit re-opens).
2. Log circuit state transitions at INFO level so operators can see when circuits open/close after restart.
3. Do NOT persist to SQLite in v1 -- the write contention with the existing fire-and-forget logging pattern (storage/logging.rs uses spawn for writes) would add complexity with minimal benefit for a single-instance local proxy.
4. If persistence is needed later, use a separate lightweight mechanism (not the main request log DB). A simple JSON file or a dedicated SQLite table with WAL mode would work.

**Phase to address:** Explicitly scope as out-of-scope for v1 circuit breaker. Document as a known limitation.

**Confidence:** HIGH -- the current architecture is a single-instance local proxy (no multi-instance coordination needed). The in-memory approach matches the codebase's existing patterns (Router state, AppState are all in-memory).

---

### Pitfall 8: /health Endpoint Reports "ok" When All Providers Are Circuit-Broken

**What goes wrong:** The current health endpoint (handlers.rs lines 876-881) returns a static `{"status": "ok"}`. After adding circuit breakers, this endpoint is the primary observability surface for external monitoring. If /health still returns 200/ok when all providers for a given model have open circuits, monitoring systems have no signal that the proxy is effectively non-functional.

But the inverse is also wrong: if /health returns unhealthy (503) when ANY provider is circuit-broken, monitoring systems will page for a single provider being down (which is handled by fallback and is not an outage).

**Why it happens:** "Health" is ambiguous. The proxy process is healthy (it can accept HTTP requests). But the proxy's ability to fulfill its purpose (route to providers) is degraded. Without a clear definition of what "healthy" means, the endpoint reports at the wrong level of abstraction.

**Consequences:**
- Kubernetes/systemd health checks mark the proxy as healthy when it cannot serve any requests
- Operators have no automated signal for "all providers down"
- Alternatively, if /health is too sensitive, it triggers false positives and gets ignored

**Prevention:**
1. Define three health levels:
   - **healthy** (HTTP 200): at least one provider per configured model has a closed circuit
   - **degraded** (HTTP 200 with `"status": "degraded"`): some providers have open circuits but every model still has at least one available provider
   - **unhealthy** (HTTP 503): at least one configured model has NO available providers (all circuits open)
2. Include per-provider circuit state in the response body:
   ```json
   {
     "status": "degraded",
     "service": "arbstr",
     "providers": {
       "provider-alpha": { "circuit": "open", "since": "2026-02-16T14:30:00Z", "failures": 3 },
       "provider-beta": { "circuit": "closed", "failures": 0 }
     },
     "models": {
       "gpt-4o": { "available_providers": 1, "total_providers": 2 },
       "claude-3.5-sonnet": { "available_providers": 0, "total_providers": 1 }
     }
   }
   ```
3. Liveness probe (Kubernetes) should hit a separate `/livez` endpoint that just checks process health (always 200 if the server is running). Readiness probe should hit `/health` with the circuit-aware logic.
4. The /health endpoint must NOT take the circuit breaker Mutex for every call. Instead, read the current state snapshot. Since the Mutex hold time is nanoseconds (see Pitfall 2 prevention), this is fine -- but avoid calling /health in a tight polling loop that could create contention.

**Phase to address:** /health endpoint enhancement phase, immediately after or concurrently with circuit breaker implementation.

**Confidence:** HIGH -- the existing /health endpoint is trivially static (handlers.rs line 876). The enhancement is purely additive. The three-level model (healthy/degraded/unhealthy) is used by Envoy, Linkerd, Kong, and other production proxies.

---

### Pitfall 9: Circuit Timer Races With Tokio Time

**What goes wrong:** The circuit breaker needs a "opened_at" timestamp to determine when 30s have elapsed for the Open-to-Half-Open transition. Using `std::time::Instant` works correctly in production but fails in tests that use `tokio::time::pause()` / `start_paused = true`. The existing tests in retry.rs (line 302: `#[tokio::test(start_paused = true)]`) rely on Tokio's mock time to test backoff delays without real waits. If the circuit breaker uses `std::time::Instant`, the 30s timeout becomes a real 30-second wall-clock wait in tests.

Conversely, using `tokio::time::Instant` in the circuit breaker struct creates a dependency on the Tokio runtime being active, which may not be the case in unit tests that do not use `#[tokio::test]`.

**Why it happens:** The codebase already mixes both: `std::time::Instant` is used for latency measurement (handlers.rs line 135), while `tokio::time::Instant` is used for the retry timeout deadline (handlers.rs line 299). The circuit breaker timeout is similar to the retry deadline but lives in a different struct.

**Consequences:**
- Tests that use `start_paused = true` wait 30 real seconds for circuit timeout, making the test suite slow
- Tests that skip the Tokio runtime (pure unit tests) panic on `tokio::time::Instant::now()` because there is no runtime context
- Flaky tests if the timer comparison uses `>` vs `>=` and the mock clock lands exactly on the boundary

**Prevention:**
1. Use `tokio::time::Instant` for the `opened_at` timestamp in the circuit breaker, matching the existing `timeout_at` usage in handlers.rs.
2. All circuit breaker tests that involve time should use `#[tokio::test(start_paused = true)]`, consistent with the existing retry backoff tests.
3. For pure unit tests of the state machine logic (not involving time), accept the timestamps as parameters rather than calling `Instant::now()` inside the state machine. This makes the state machine testable without a runtime.
4. Use `tokio::time::sleep` in the `start_paused = true` tests and `tokio::time::advance` to fast-forward through the 30s timeout in milliseconds.

**Phase to address:** Circuit breaker state machine implementation and test design.

**Confidence:** HIGH -- the existing test suite demonstrates both time patterns (retry.rs uses `start_paused`, handlers.rs uses `std::time::Instant`). The pitfall is specific to this codebase's testing conventions.

---

## Minor Pitfalls

Issues that cause suboptimal behavior or minor inconsistencies but do not break correctness.

---

### Pitfall 10: Forgetting to Reset Failure Counter on Success

**What goes wrong:** In the Closed state, the failure counter tracks consecutive failures. When a request succeeds, the counter must reset to zero. If the reset is forgotten (easy to miss because success is the "happy path" that gets less testing), the counter ratchets up over time: fail, fail, succeed, fail, fail, fail -- with correct reset, the max consecutive is 3 (trips circuit). Without reset, the counter reaches 6 and would have tripped at 3 even though successes intervened.

**Why it happens:** The success path through `send_to_provider` returns `Ok(RequestOutcome)`. Developers add `record_failure` in the error path but forget `record_success` in the success path because "nothing needs to happen on success."

**Consequences:**
- Circuit trips on accumulated non-consecutive failures
- Provider appears less healthy than it actually is
- Flapping behavior: circuit opens, probe succeeds (closes), a single failure immediately re-opens because the counter was not properly reset

**Prevention:**
1. The `record_success` method must exist and must be called on every successful response. It should: (a) reset failure_count to 0, (b) in Half-Open state, transition to Closed.
2. In code review, verify that every code path through `send_to_provider` that returns `Ok(...)` also calls `record_success`.
3. Write a specific test: "after N-1 failures and 1 success, the next N-1 failures should not trip the circuit." This directly catches missing resets.

**Phase to address:** Core circuit breaker implementation. Must be in the initial PR, not a follow-up.

**Confidence:** HIGH -- this is the most commonly cited circuit breaker implementation bug in tutorials and production post-mortems.

---

### Pitfall 11: Circuit Breaker Per-Provider-Name vs Per-Provider-Model

**What goes wrong:** A provider might serve multiple models (e.g., "provider-alpha" serves gpt-4o and claude-3.5-sonnet). If the circuit breaker is keyed by provider name only, a failure for gpt-4o trips the circuit for claude-3.5-sonnet too, even though that model might work fine on the same provider.

Conversely, if the circuit is keyed by (provider, model), the state space grows (N providers x M models) and a provider that is genuinely down (network-level failure) requires separate circuits for each model to trip independently, which is slower.

**Why it happens:** The router already selects by model, so by the time a request reaches the provider, the model is fixed. The question is whether a circuit for "provider-alpha" means "provider-alpha is unhealthy" or "provider-alpha serving gpt-4o is unhealthy."

**Consequences:**
- Per-provider key: one bad model on a provider takes down all models (false positive)
- Per-provider-model key: connection-level failures need N trips to fully circuit-break a provider (slow detection)

**Prevention:**
1. Use per-provider-name keying for v1. A provider being down (connection failure, 502, 503) affects all models equally because the failure is at the HTTP/network level, not the model level.
2. Model-specific issues (500 error for a specific model) are less common in the Routstr marketplace context and would need model-specific error detection that does not exist yet.
3. Document this as a known limitation: "per-model circuit breakers are a future enhancement for when model-level health signals are available."

**Phase to address:** Circuit breaker key design. Decide early, but per-provider is the correct v1 scope.

**Confidence:** MEDIUM -- this depends on how Routstr marketplace providers actually behave. If providers run different models on different infrastructure, per-model would be more accurate. But the current config structure (ProviderConfig has a single url for all models) suggests they share infrastructure.

---

### Pitfall 12: Existing 30-Second RETRY_TIMEOUT Conflicts With 30-Second Circuit Open Duration

**What goes wrong:** The existing `RETRY_TIMEOUT` in handlers.rs (line 39) is 30 seconds. The circuit breaker open duration is also specified as 30 seconds. If a request starts, triggers a circuit-open on the primary provider, falls back to another provider that is also circuit-broken, the request will wait up to 30 seconds for the circuits to expire. But the RETRY_TIMEOUT is also 30 seconds, creating a tight race between "circuit might reopen" and "request times out."

In the worst case: all providers are circuit-broken, a request arrives, and the client waits the full 30s before getting a 504. This defeats the circuit breaker's purpose of "fail fast."

**Why it happens:** Both constants were chosen independently with the same round number. The retry timeout was designed for "how long to let retries + backoff run" (which with 3 retries at 1s+2s is only 7s of actual wait, well under 30s). The circuit open duration was chosen for "how long to wait before probing" (standard recommendation). They were not designed to interact.

**Consequences:**
- "All providers circuit-open" scenario is not fail-fast (waits up to 30s before 503/504)
- The 503 from circuit-open is indistinguishable from the 504 timeout in timing

**Prevention:**
1. When all candidates have open circuits, return 503 immediately (see Pitfall 4). Do NOT enter the retry loop at all.
2. The circuit check should happen BEFORE entering `retry_with_fallback`, as a pre-check: "are there any non-open-circuit candidates?" If no, fail fast with 503 + Retry-After.
3. If SOME candidates are open and some are closed, enter retry_with_fallback with only the non-open candidates. This avoids wasting time on open circuits.
4. Consider reducing RETRY_TIMEOUT to 15 seconds now that circuit breakers provide fast failure for downed providers. The 30s timeout was needed when every failure required actual request + backoff. With circuit breakers, most failures are instant rejections.

**Phase to address:** Integration phase where circuit breaker is wired into the request path. The timeout adjustment can be a separate follow-up.

**Confidence:** HIGH -- both constants are visible in the code (RETRY_TIMEOUT at handlers.rs:39, circuit open duration is a design parameter). The conflict is arithmetic.

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|---|---|---|
| State machine design | Pitfall 2 (atomic race), Pitfall 5 (half-open race) | Use `Mutex<CircuitState>` with single-permit half-open model |
| Retry integration | Pitfall 1 (amplification), Pitfall 12 (timeout conflict) | Circuit check inside send_to_provider; circuit-open is non-retryable but triggers fallback; pre-check before retry loop |
| Failure recording | Pitfall 6 (4xx counting), Pitfall 10 (missing success reset) | Reuse `is_retryable` logic; mandate `record_success` on every Ok path |
| Streaming support | Pitfall 3 (streaming bypass) | Record circuit failures in spawned stream completion task; Arc<Mutex<CircuitState>> is Send + Sync |
| Router integration | Pitfall 4 (NoProviders error), Pitfall 11 (key granularity) | New error variant for all-circuit-open; per-provider-name keying for v1 |
| /health endpoint | Pitfall 8 (static health) | Three-level health model; per-provider circuit state in response body |
| Testing | Pitfall 9 (timer in tests) | Use tokio::time::Instant; start_paused tests; parameterize state machine |
| Persistence | Pitfall 7 (restart resets) | Accept in-memory for v1; log transitions at INFO; document limitation |

---

## Sources

- [Circuit Breaker Pattern - Azure Architecture Center (Microsoft)](https://learn.microsoft.com/en-us/azure/architecture/patterns/circuit-breaker) -- concurrency considerations, half-open state design, resource differentiation
- [Retry Storm Antipattern - Azure Architecture Center (Microsoft)](https://learn.microsoft.com/en-us/azure/architecture/antipatterns/retry-storm/) -- retry amplification, circuit breaker as mitigation
- [Circuit Breaker - Martin Fowler](https://martinfowler.com/bliki/CircuitBreaker.html) -- canonical pattern description, state transitions
- [Resilience4j CircuitBreaker Documentation](https://resilience4j.readme.io/docs/circuitbreaker) -- atomic state management, sliding window, half-open permit model
- [tower-circuitbreaker crate](https://docs.rs/tower-circuitbreaker/latest/tower_circuitbreaker/) -- Rust-specific async circuit breaker middleware
- [Rust Atomics and Locks (Mara Bos)](https://marabos.nl/atomics/) -- atomic operation correctness, CAS patterns, when to use Mutex vs atomics
- [Kong Gateway Health Checks](https://docs.konghq.com/gateway/latest/how-kong-works/health-checks/) -- per-provider health tracking in proxy architecture
- [Envoy Health Checking](https://www.envoyproxy.io/docs/envoy/latest/intro/arch_overview/upstream/health_checking) -- degraded health status model
- Direct codebase analysis: `src/proxy/retry.rs`, `src/proxy/handlers.rs`, `src/proxy/stream.rs`, `src/proxy/server.rs`, `src/router/selector.rs`, `src/error.rs`
