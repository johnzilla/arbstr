# Architecture Patterns: Per-Provider Circuit Breaker and Enhanced /health

**Domain:** Resilience layer for existing Rust/axum/tokio LLM proxy
**Researched:** 2026-02-16
**Overall confidence:** HIGH (patterns verified against existing codebase, circuit breaker is a well-established pattern, integration points identified through direct code inspection)

## Current Architecture (Baseline)

### Existing Component Map

```
src/
  proxy/
    server.rs     -- AppState { router, http_client, config, db, read_db }
                     create_router() builds axum::Router with routes + state
    handlers.rs   -- chat_completions, list_models, health, list_providers, stats, logs
    retry.rs      -- retry_with_fallback(), AttemptRecord, CandidateInfo, is_retryable()
    stream.rs     -- SseObserver, wrap_sse_stream(), StreamResultHandle
    stats.rs      -- /v1/stats handler
    logs.rs       -- /v1/requests handler
    types.rs      -- OpenAI-compatible request/response types
    mod.rs        -- declares modules, re-exports public items
  router/
    selector.rs   -- Router, SelectedProvider, select(), select_candidates()
  storage/
    mod.rs        -- init_pool(), init_read_pool()
    logging.rs    -- RequestLog, spawn_log_write(), spawn_stream_completion_update()
    stats.rs      -- Aggregate queries for /v1/stats
    logs.rs       -- Paginated queries for /v1/requests
  config.rs       -- Config, ProviderConfig, PolicyRule
  error.rs        -- Error enum with IntoResponse
  lib.rs          -- pub mod declarations
```

### Current Request Flow (What Circuit Breaker Must Integrate With)

The non-streaming chat completions path is the critical integration point:

```
1. chat_completions handler receives request
2. router.select_candidates() returns Vec<SelectedProvider> sorted cheapest-first
3. CandidateInfo list built from candidates (just name field)
4. retry_with_fallback(candidates, attempts, send_request) called within 30s timeout
5. retry module: tries primary up to 3 times (1s, 2s backoff), then fallback once
6. send_to_provider() makes HTTP call to selected provider
7. Result logged to SQLite
```

Key observations for circuit breaker integration:

- **Provider selection is pure:** `select_candidates()` filters by model/policy/cost only. It has no awareness of provider health. This is where circuit breaker filtering must inject.
- **Retry is per-request:** The retry module iterates over a pre-selected candidate list. It does not consult any global state about provider health. Failed attempts are recorded in a per-request `Arc<Mutex<Vec<AttemptRecord>>>`.
- **Fallback is limited:** Only the first two candidates are ever tried (primary + 1 fallback). The circuit breaker should prevent known-bad providers from even appearing in this list.
- **Streaming path has no retry:** The streaming path calls `execute_request()` which uses `router.select()` (single best provider, no fallback). This path also benefits from circuit breaker filtering.
- **AppState is Clone + shared via axum:** All handlers receive `State<AppState>`. The circuit breaker state must live inside `AppState` or be reachable from it.

### How Providers Are Identified

Providers are identified by their `name: String` field throughout the codebase:
- `ProviderConfig.name` (config)
- `SelectedProvider.name` (router output)
- `CandidateInfo.name` (retry module)
- `AttemptRecord.provider_name` (failure tracking)
- `RequestLog.provider` (database)

The circuit breaker should key on provider name, consistent with the rest of the codebase.

---

## Recommended Architecture

### New Component: `src/proxy/circuit_breaker.rs`

**Decision: Create a new module `src/proxy/circuit_breaker.rs` for the circuit breaker state machine.**

Rationale:
- The circuit breaker is a proxy-layer concern -- it determines whether a provider should be tried, sitting between the router (selection) and the HTTP call (execution)
- It does not belong in `router/` because the router is about cost/model/policy selection, not health tracking
- It does not belong in `storage/` because it is in-memory state, not persistent data
- It lives alongside `retry.rs` because both are resilience mechanisms in the proxy layer

### Modified Components

| File | Change | What |
|------|--------|------|
| `src/proxy/circuit_breaker.rs` | **NEW** | `CircuitBreakerRegistry` (shared state), `ProviderCircuitBreaker` (per-provider state machine), `CircuitState` enum |
| `src/proxy/server.rs` | MODIFY | Add `circuit_breakers: Arc<CircuitBreakerRegistry>` to `AppState`, initialize in `run_server()` |
| `src/proxy/handlers.rs` | MODIFY | After request success/failure, call `circuit_breakers.record_success/failure()`. Filter candidates through circuit breaker before retry. Enhanced `/health` handler. |
| `src/proxy/mod.rs` | MODIFY | Add `pub mod circuit_breaker;` declaration |
| `src/error.rs` | MODIFY | Add `Error::CircuitOpen { provider: String }` variant (optional, for logging clarity) |

**No changes to:** `router/selector.rs`, `retry.rs`, `config.rs`, `storage/`, `stream.rs`.

This is a critical design constraint: the router and retry modules remain untouched. The circuit breaker wraps around them at the handler level.

### Component Boundaries

```
                           chat_completions handler
                                    |
                    1. router.select_candidates(model, policy, prompt)
                                    |
                          Vec<SelectedProvider>
                                    |
                    2. circuit_breakers.filter_available(candidates)
                                    |
                     Vec<SelectedProvider> (healthy only)
                                    |
                    3. retry_with_fallback(filtered_candidates, ...)
                                    |
                    4. send_to_provider(provider, request)
                                    |
                          success or failure
                                    |
                    5. circuit_breakers.record_outcome(provider_name, success/failure)
```

```
+-------------------------------------------+
|  proxy/circuit_breaker.rs                  |
|                                            |
|  CircuitBreakerRegistry                    |
|    breakers: DashMap<String, ProviderCB>   |
|    config: CircuitBreakerConfig            |
|                                            |
|    fn filter_available(&self,              |
|       candidates: Vec<SelectedProvider>)   |
|       -> Vec<SelectedProvider>             |
|                                            |
|    fn record_success(&self, provider: &str)|
|    fn record_failure(&self, provider: &str)|
|    fn provider_states(&self)               |
|       -> Vec<ProviderHealthInfo>           |
|                                            |
|  ProviderCircuitBreaker                    |
|    state: CircuitState                     |
|    failure_count: u32                      |
|    last_failure: Option<Instant>           |
|    success_count: u32                      |
|    last_state_change: Instant              |
|                                            |
|  CircuitState { Closed, Open, HalfOpen }   |
|  CircuitBreakerConfig                      |
|    failure_threshold: u32                  |
|    open_duration: Duration                 |
|    half_open_max_calls: u32               |
+-------------------------------------------+
          |                    |
    used by handlers     used by /health
```

### Where It Does NOT Live

The circuit breaker does NOT modify the `Router` in `selector.rs`. This is deliberate:

- The router answers "which providers can serve this model at what cost?" -- a static question based on config
- The circuit breaker answers "which of those providers are currently healthy?" -- a dynamic runtime question
- Mixing these concerns would make the router stateful and harder to test
- The filtering happens in the handler, between selection and retry, which is the natural composition point

### Why Not a Tower Middleware/Layer?

Tower middleware would intercept at the HTTP layer, wrapping individual `send_to_provider` calls. This is the wrong abstraction because:

1. The circuit breaker needs to **filter the candidate list before retry starts**, not fail individual calls
2. Tower layers operate on individual requests, not on the routing decision
3. The retry module already handles per-call failure logic -- the circuit breaker operates at a higher level (provider availability)
4. Adding a Tower layer would require restructuring the retry module, violating the "no changes to retry.rs" constraint

---

## Data Structures

### CircuitState Enum

```rust
/// The three canonical circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation. Requests flow through. Failures are counted.
    Closed,
    /// Provider is considered unhealthy. Requests are blocked.
    /// Transitions to HalfOpen after `open_duration` elapses.
    Open,
    /// Probing state. A limited number of requests are allowed through.
    /// Success -> Closed. Failure -> Open.
    HalfOpen,
}
```

### ProviderCircuitBreaker

```rust
/// Per-provider circuit breaker state machine.
///
/// All fields are plain (non-atomic) because access is serialized through
/// DashMap's per-key lock. No interior mutability needed.
#[derive(Debug, Clone)]
pub struct ProviderCircuitBreaker {
    state: CircuitState,
    /// Consecutive failure count in Closed state. Reset on success.
    failure_count: u32,
    /// When the circuit last transitioned to Open state.
    opened_at: Option<Instant>,
    /// Number of successful probe calls in HalfOpen state.
    half_open_successes: u32,
    /// Timestamp of last state transition (for observability).
    last_state_change: Instant,
}
```

**Why plain fields, not atomics:** `DashMap` provides per-shard locking. When we access a provider's entry via `breakers.get_mut("provider-name")`, we hold an exclusive lock on that shard. No concurrent mutation is possible for that key, so atomics would be redundant overhead.

### CircuitBreakerConfig

```rust
/// Configuration for circuit breaker behavior.
///
/// Hardcoded initially (not in config.toml). Can be promoted to config later
/// if users need to tune these values.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    /// Default: 5
    pub failure_threshold: u32,
    /// How long the circuit stays open before transitioning to HalfOpen.
    /// Default: 30 seconds
    pub open_duration: Duration,
    /// Number of successful probes needed in HalfOpen to close the circuit.
    /// Default: 2
    pub half_open_success_threshold: u32,
}
```

**Why hardcoded initially:** The circuit breaker is new. Getting the thresholds right requires operational experience. Hardcoding with good defaults avoids premature config surface area. Constants at the top of the file make them easy to find and change. Promote to `config.toml` in a future milestone if users request tuning.

### CircuitBreakerRegistry

```rust
use dashmap::DashMap;

/// Shared registry of per-provider circuit breakers.
///
/// Thread-safe via DashMap's internal sharded locking. Cheap to clone
/// (DashMap is internally Arc'd). Created once at startup and shared
/// through AppState.
pub struct CircuitBreakerRegistry {
    breakers: DashMap<String, ProviderCircuitBreaker>,
    config: CircuitBreakerConfig,
}
```

**Why DashMap over RwLock<HashMap>:** DashMap provides per-shard locking, meaning recording a failure for "provider-alpha" does not block checking "provider-beta". With `RwLock<HashMap>`, any mutation takes a write lock that blocks all readers. For a proxy handling concurrent requests to different providers, this contention matters. DashMap is a well-established crate (170M+ downloads) and adds minimal dependency weight.

**Why not per-provider Mutex:** A `HashMap<String, Mutex<ProviderCircuitBreaker>>` requires a read lock on the outer HashMap to access any provider, then a Mutex lock on the inner state. Two lock acquisitions per operation. DashMap does this in one step with better ergonomics.

---

## State Machine Transitions

```
              success
    +--------+-------+
    |        |       |
    v        |       |
 CLOSED -----+    HALF-OPEN
    |                ^  |
    | failure_count  |  | failure
    | >= threshold   |  |
    |                |  v
    +-----> OPEN ----+
         (timeout expires)
```

### Transition Rules

| From | To | Trigger |
|------|----|---------|
| Closed | Open | `failure_count >= failure_threshold` (consecutive) |
| Open | HalfOpen | `Instant::now() >= opened_at + open_duration` |
| HalfOpen | Closed | `half_open_successes >= half_open_success_threshold` |
| HalfOpen | Open | Any failure during probing |
| Closed | Closed | Success (resets `failure_count` to 0) |
| Open | Open | No-op (requests blocked, time not elapsed) |

### The "Time-Based Transition" Design Decision

The Open -> HalfOpen transition is **checked lazily** when `is_available()` is called, not via a background timer. This means:

1. `is_available("provider-alpha")` checks: is state Open AND has `open_duration` elapsed?
2. If yes: transition to HalfOpen and return `true` (allow one probe)
3. If no: return `false` (still blocked)

**Why lazy, not timer-based:**
- No background tasks to manage or cancel
- No risk of timer drift or missed state transitions
- State transition happens exactly when needed (at request time)
- Simpler to test (control time via `tokio::time::pause()`)
- If no requests come in while a provider is Open, there is no wasted probe -- the transition happens on the next actual request

---

## Integration with Existing Handler Flow

### Non-Streaming Path (Primary Integration Point)

Current flow in `chat_completions` (handlers.rs, lines 234-474):

```rust
// Current: get candidates from router
let candidates = state.router.select_candidates(&request.model, ...)?;

// Current: build CandidateInfo for retry module
let candidate_infos: Vec<CandidateInfo> = candidates.iter()
    .map(|c| CandidateInfo { name: c.name.clone() })
    .collect();

// Current: retry with fallback
let timeout_result = timeout_at(deadline,
    retry_with_fallback(&candidate_infos, attempts.clone(), |info| {
        send_to_provider(&state, &request, provider, &correlation_id, false)
    })
).await;
```

Modified flow (additions marked with `// NEW`):

```rust
// Unchanged: get candidates from router
let candidates = state.router.select_candidates(&request.model, ...)?;

// NEW: filter through circuit breaker
let candidates = state.circuit_breakers.filter_available(candidates);

// NEW: handle case where all providers are circuit-broken
if candidates.is_empty() {
    // Return 503 Service Unavailable with circuit breaker info
    // (distinct from the router's 400 "no providers for model")
    return Ok(service_unavailable_response(...));
}

// Unchanged: build CandidateInfo for retry module
let candidate_infos: Vec<CandidateInfo> = candidates.iter()
    .map(|c| CandidateInfo { name: c.name.clone() })
    .collect();

// Unchanged: retry with fallback (retry.rs is NOT modified)
let timeout_result = timeout_at(deadline,
    retry_with_fallback(&candidate_infos, attempts.clone(), |info| {
        send_to_provider(&state, &request, provider, &correlation_id, false)
    })
).await;

// NEW: record outcome based on retry result
match &timeout_result {
    Ok(retry_outcome) => match &retry_outcome.result {
        Ok(outcome) => {
            state.circuit_breakers.record_success(&outcome.provider_name);
        }
        Err(outcome_err) => {
            if let Some(ref name) = outcome_err.provider_name {
                state.circuit_breakers.record_failure(name);
            }
        }
    },
    Err(_timeout) => {
        // Record failures for all providers that were attempted
        for attempt in recorded_attempts.iter() {
            state.circuit_breakers.record_failure(&attempt.provider_name);
        }
    }
}
```

### Streaming Path

The streaming path uses `execute_request()` which calls `router.select()` (single provider). Modification:

```rust
// In execute_request(), after provider selection:
let provider = state.router.select(&request.model, policy_name, user_prompt)?;

// NEW: check circuit breaker for this specific provider
if !state.circuit_breakers.is_available(&provider.name) {
    // Try select_candidates and filter, fall back to error
    let candidates = state.router.select_candidates(&request.model, policy_name, user_prompt)?;
    let available = state.circuit_breakers.filter_available(candidates);
    let provider = available.into_iter().next().ok_or_else(|| {
        RequestError { /* all providers circuit-broken */ }
    })?;
}
```

### Recording Granularity: Per-Request, Not Per-Attempt

The circuit breaker records the **final outcome** of the request for a provider, not every retry attempt. This is intentional:

- The retry module already handles transient failures (503 -> retry with backoff)
- A provider that returns 503 once but succeeds on retry is not unhealthy
- Only persistent failures (all retries exhausted) should count toward the circuit breaker threshold
- This prevents a single slow response from prematurely tripping the circuit

However, **timeouts are an exception**: if the 30s deadline expires, we should record failures for all attempted providers, because the proxy has evidence of systemic unavailability.

---

## Enhanced /health Endpoint

### Current Implementation

```rust
pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "arbstr"
    }))
}
```

### Enhanced Implementation

```rust
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let provider_health = state.circuit_breakers.provider_states();
    let db_healthy = match &state.db {
        Some(pool) => sqlx::query("SELECT 1").fetch_one(pool).await.is_ok(),
        None => false,
    };

    let all_providers_healthy = provider_health.iter()
        .all(|p| p.state == CircuitState::Closed);

    let overall_status = if !db_healthy {
        "degraded"
    } else if !all_providers_healthy {
        "degraded"
    } else {
        "ok"
    };

    // Use 200 for ok/degraded, only 503 if completely non-functional
    let status_code = StatusCode::OK;

    Json(serde_json::json!({
        "status": overall_status,
        "service": "arbstr",
        "components": {
            "database": {
                "status": if db_healthy { "ok" } else { "unavailable" }
            },
            "providers": provider_health.iter().map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "circuit_state": format!("{:?}", p.state),
                    "failure_count": p.failure_count,
                    "last_state_change_secs_ago": p.last_state_change_secs_ago,
                })
            }).collect::<Vec<_>>()
        }
    }))
}
```

### ProviderHealthInfo

```rust
/// Read-only snapshot of a provider's circuit breaker state for the /health endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderHealthInfo {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
    pub last_state_change_secs_ago: u64,
}
```

The `provider_states()` method on `CircuitBreakerRegistry` iterates over the DashMap and produces a `Vec<ProviderHealthInfo>` snapshot. This is a read-only operation that holds each shard lock briefly.

### Health Endpoint Design Decisions

1. **Always return 200 OK** even when degraded. Kubernetes/monitoring tools should use the `status` field value, not the HTTP status code. A 503 from `/health` would cause load balancers to remove arbstr itself, which is the wrong response to a provider being unhealthy.

2. **Include circuit breaker states per provider.** This is the primary diagnostic value: operators can see which providers are open, how many failures triggered it, and how long ago the state changed.

3. **Include database status.** A simple `SELECT 1` ping. If it fails, stats/logging are degraded but proxying still works.

4. **Do not include latency metrics in /health.** Those belong in `/v1/stats`. The health endpoint answers "is the system functional?" not "how is it performing?"

---

## AppState Changes

### Current

```rust
#[derive(Clone)]
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: Option<SqlitePool>,
    pub read_db: Option<SqlitePool>,
}
```

### Modified

```rust
#[derive(Clone)]
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: Option<SqlitePool>,
    pub read_db: Option<SqlitePool>,
    pub circuit_breakers: Arc<CircuitBreakerRegistry>,  // NEW
}
```

**Why `Arc<CircuitBreakerRegistry>`:** `AppState` must be `Clone` (axum requirement). `DashMap` is internally `Arc`'d, but `CircuitBreakerRegistry` also holds `CircuitBreakerConfig`. Wrapping in `Arc` makes the whole thing cheaply cloneable. `Clone` on `Arc` is just a reference count increment.

### Initialization in `run_server()`

```rust
pub async fn run_server(config: Config) -> anyhow::Result<()> {
    // ... existing setup ...

    // NEW: Initialize circuit breaker registry with one entry per configured provider
    let circuit_breakers = Arc::new(CircuitBreakerRegistry::new(
        CircuitBreakerConfig::default(),
        config.providers.iter().map(|p| p.name.clone()),
    ));

    let state = AppState {
        router: Arc::new(provider_router),
        http_client,
        config: Arc::new(config),
        db,
        read_db,
        circuit_breakers,  // NEW
    };
    // ...
}
```

Pre-populating the registry with known provider names avoids lazy initialization and ensures `/health` reports all providers from the start, even before any requests are made.

---

## New Dependency: DashMap

Add to `Cargo.toml`:

```toml
dashmap = "6"
```

DashMap v6 is the current stable release. It has no transitive dependencies beyond `hashbrown` (which is already a transitive dependency of many crates in the tree). Minimal dependency weight.

**Alternatives considered:**
- `std::sync::RwLock<HashMap>`: Write-lock contention on any state mutation. Poor for concurrent multi-provider proxying.
- `tokio::sync::RwLock<HashMap>`: Async lock, but unnecessary -- circuit breaker operations are fast (no I/O), so sync locks are preferred to avoid holding locks across `.await` points.
- No external dependency (inline sharded map): More code to maintain, DashMap is battle-tested.

---

## Anti-Patterns to Avoid

### Anti-Pattern 1: Circuit Breaker Inside the Router

**What:** Adding health state to `Router` in `selector.rs`, making `select_candidates()` filter by circuit state.
**Why bad:** Violates single responsibility. The router becomes stateful, harder to test, and the circuit breaker logic cannot be tested independently.
**Instead:** Keep the router pure (model/cost/policy). Filter at the handler level.

### Anti-Pattern 2: Recording Every Retry Attempt

**What:** Calling `record_failure()` on every failed attempt within `retry_with_fallback()`.
**Why bad:** A provider that returns 503 once then succeeds on retry is not unhealthy. Recording all attempts would trip the circuit prematurely.
**Instead:** Record only the final outcome per provider per request. This means the retry module (`retry.rs`) is not modified.

### Anti-Pattern 3: Background Health Probes

**What:** Spawning a `tokio::spawn` loop that periodically pings providers to check if they are alive.
**Why bad:** Adds complexity (task management, cancellation), may not use the same code path as real requests, and wastes resources when no requests are being made.
**Instead:** Use lazy transition (Open -> HalfOpen checked on next request). The "probe" is a real request, which is the most accurate health signal.

### Anti-Pattern 4: Mutex Around the Entire Registry

**What:** Using `Mutex<HashMap<String, ProviderCircuitBreaker>>` for the registry.
**Why bad:** Any mutation (recording a failure for provider A) blocks all reads (checking availability of provider B). Under concurrent load, this serializes all circuit breaker operations.
**Instead:** DashMap with per-shard locking. Operations on different providers do not contend.

### Anti-Pattern 5: Making the Circuit Breaker Async

**What:** Using `tokio::sync::Mutex` or `tokio::sync::RwLock` for circuit breaker state.
**Why bad:** Circuit breaker operations are pure state machine transitions (compare timestamps, increment counters). They complete in nanoseconds. Async locks are designed for operations that hold locks across `.await` points. Using async locks here adds unnecessary overhead and requires all callers to `.await` the lock.
**Instead:** Synchronous `DashMap` access. The lock is held for microseconds.

---

## Patterns to Follow

### Pattern 1: Lazy State Transition

**What:** Check and transition Open -> HalfOpen inside `is_available()`, not via a timer.
**When:** Every call to `filter_available()` or `is_available()`.
**Example:**

```rust
impl ProviderCircuitBreaker {
    fn is_available(&mut self, config: &CircuitBreakerConfig) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(opened_at) = self.opened_at {
                    if Instant::now().duration_since(opened_at) >= config.open_duration {
                        // Transition to HalfOpen
                        self.state = CircuitState::HalfOpen;
                        self.half_open_successes = 0;
                        self.last_state_change = Instant::now();
                        tracing::info!(provider = %"...", "Circuit breaker: Open -> HalfOpen");
                        true  // Allow one probe request
                    } else {
                        false  // Still blocked
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,  // Allow probe requests
        }
    }
}
```

### Pattern 2: Structured Logging on State Transitions

**What:** Log at `info` level on every state transition, `debug` on every recorded event.
**When:** Every call to `record_success()`, `record_failure()`, or state transition.
**Example:**

```rust
tracing::info!(
    provider = %name,
    from = ?old_state,
    to = ?new_state,
    failure_count = breaker.failure_count,
    "Circuit breaker state transition"
);
```

This is critical for observability. Operators need to know when a provider trips and recovers without checking `/health` constantly.

### Pattern 3: Filter, Don't Reject

**What:** The circuit breaker filters the candidate list rather than rejecting individual requests.
**When:** Before building the `CandidateInfo` list for retry.
**Why:** This naturally falls back to the next cheapest provider. If provider A is circuit-broken but provider B is healthy, the request succeeds transparently. The caller (user's application) never sees a circuit breaker error unless ALL providers are down.

### Pattern 4: Response Header for Circuit Breaker Events

**What:** Add `x-arbstr-circuit-state: provider-name=open` header when a provider was skipped due to circuit breaker.
**When:** Any request where circuit breaker filtering removed candidates.
**Why:** Gives client applications visibility into routing decisions without checking `/health`.

---

## Build Order

Implement in this sequence, where each step depends on the previous:

### Phase 1: Circuit Breaker Core (no handler changes)

1. **Add `dashmap = "6"` to Cargo.toml**
2. **Create `src/proxy/circuit_breaker.rs`:**
   - `CircuitState` enum
   - `CircuitBreakerConfig` with `Default` impl
   - `ProviderCircuitBreaker` struct with state machine methods:
     - `is_available(&mut self, config) -> bool`
     - `record_success(&mut self, config)` (transitions HalfOpen -> Closed)
     - `record_failure(&mut self, config)` (transitions Closed -> Open, HalfOpen -> Open)
   - `CircuitBreakerRegistry` struct:
     - `new(config, provider_names) -> Self`
     - `filter_available(&self, candidates) -> Vec<SelectedProvider>`
     - `record_success(&self, provider_name)`
     - `record_failure(&self, provider_name)`
     - `provider_states(&self) -> Vec<ProviderHealthInfo>`
     - `is_available(&self, provider_name) -> bool`
   - `ProviderHealthInfo` struct for /health
   - Unit tests for all state transitions (use `tokio::time::pause()` for time-dependent tests)
3. **Update `src/proxy/mod.rs`:** Add `pub mod circuit_breaker;`

**Tests at this point:** Pure unit tests. The circuit breaker module has no dependencies on axum, reqwest, or sqlx. All methods are synchronous. Test state transitions, threshold counting, timeout behavior.

### Phase 2: AppState Integration

4. **Modify `src/proxy/server.rs`:**
   - Add `circuit_breakers: Arc<CircuitBreakerRegistry>` to `AppState`
   - Initialize in `run_server()` from config provider names
   - For `--mock` mode: initialize with mock provider names

**Tests at this point:** Verify `AppState` still constructs and clones correctly.

### Phase 3: Handler Integration (Non-Streaming)

5. **Modify `src/proxy/handlers.rs` (non-streaming path):**
   - After `select_candidates()`: filter through `circuit_breakers.filter_available()`
   - Handle empty-after-filter case (503 response)
   - After retry outcome: call `record_success()` or `record_failure()`
   - Add `x-arbstr-circuit-state` response header when filtering occurs

**Tests at this point:** Integration tests. Spin up test server with mock providers, trigger failures to open circuit, verify subsequent requests skip the failed provider.

### Phase 4: Handler Integration (Streaming)

6. **Modify `src/proxy/handlers.rs` (streaming path):**
   - In `execute_request()`: check circuit breaker before using selected provider
   - Fall back to `select_candidates()` + filter if primary is circuit-broken
   - Record outcome after streaming completes (in the spawned background task)

**Tests at this point:** Integration tests for streaming path with circuit-broken providers.

### Phase 5: Enhanced /health Endpoint

7. **Modify `src/proxy/handlers.rs` (health handler):**
   - Change signature to accept `State<AppState>`
   - Add provider circuit breaker states to response
   - Add database ping
   - Return structured health response with `status`, `components`

**Tests at this point:** Integration tests for `/health` response structure, verify it reflects circuit breaker state changes.

### Phase 6: Polish and Edge Cases

8. **Add `x-arbstr-circuit-state` header to response** when circuit breaker filtering occurs
9. **Handle provider added/removed at runtime** (if applicable) -- for now, providers are static from config
10. **Add `Error::CircuitOpen` variant** to `error.rs` if needed for cleaner error messages

### Why This Order

- Phase 1 is independently testable with no integration risk
- Phase 2 is a trivial structural change (add field to struct)
- Phase 3 is the highest-value integration (non-streaming handles all retried requests)
- Phase 4 extends to streaming (lower priority because streaming already has no retry)
- Phase 5 is purely additive (new response format, no behavior change)
- Phase 6 is polish that can happen any time after Phase 3

---

## Testing Strategy

### Unit Tests (circuit_breaker.rs)

```rust
#[test]
fn test_closed_allows_requests() {
    let config = CircuitBreakerConfig::default();
    let mut cb = ProviderCircuitBreaker::new();
    assert!(cb.is_available(&config));
}

#[test]
fn test_failures_trip_circuit() {
    let config = CircuitBreakerConfig { failure_threshold: 3, ..Default::default() };
    let mut cb = ProviderCircuitBreaker::new();
    cb.record_failure(&config);
    cb.record_failure(&config);
    assert!(cb.is_available(&config)); // 2 < 3
    cb.record_failure(&config);
    assert!(!cb.is_available(&config)); // 3 >= 3, now Open
}

#[tokio::test(start_paused = true)]
async fn test_open_transitions_to_half_open_after_timeout() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(30),
        ..Default::default()
    };
    let mut cb = ProviderCircuitBreaker::new();
    cb.record_failure(&config); // -> Open
    assert!(!cb.is_available(&config));

    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(cb.is_available(&config)); // -> HalfOpen
    assert_eq!(cb.state(), CircuitState::HalfOpen);
}

#[test]
fn test_success_resets_failure_count() {
    let config = CircuitBreakerConfig { failure_threshold: 3, ..Default::default() };
    let mut cb = ProviderCircuitBreaker::new();
    cb.record_failure(&config);
    cb.record_failure(&config);
    cb.record_success(&config);
    assert_eq!(cb.failure_count(), 0); // Reset
    assert!(cb.is_available(&config));
}

#[test]
fn test_half_open_failure_reopens() {
    // ... trip to Open, advance time to HalfOpen, record failure -> Open again
}

#[test]
fn test_half_open_success_closes() {
    // ... trip to Open, advance time to HalfOpen, record enough successes -> Closed
}
```

### Registry Tests

```rust
#[test]
fn test_filter_available_removes_open_providers() {
    let registry = CircuitBreakerRegistry::new(
        CircuitBreakerConfig { failure_threshold: 1, ..Default::default() },
        ["alpha", "beta"].iter().map(|s| s.to_string()),
    );

    registry.record_failure("alpha");

    let candidates = vec![
        mock_selected_provider("alpha"),
        mock_selected_provider("beta"),
    ];

    let filtered = registry.filter_available(candidates);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "beta");
}
```

### Integration Tests (tests/circuit_breaker.rs)

Full HTTP tests using the same pattern as existing `tests/stats.rs` and `tests/logs.rs`:

```rust
// 1. Start test server with mock providers
// 2. Configure one provider to always return 503
// 3. Send enough requests to trip the circuit
// 4. Verify next request uses fallback provider
// 5. Check /health shows the tripped provider as Open
// 6. Advance time (or wait), verify HalfOpen transition
```

---

## Scalability Considerations

| Concern | At current scale (local proxy) | At scale |
|---------|-------------------------------|----------|
| DashMap contention | Negligible (few providers, microsecond locks) | Still fine -- sharding means parallel provider ops don't contend |
| Memory | ~100 bytes per provider entry | Negligible even with hundreds of providers |
| State persistence | In-memory only, reset on restart | Could persist to SQLite if restart recovery matters |
| Clock precision | `Instant::now()` is sufficient | No change needed |
| Circuit breaker thundering herd | HalfOpen allows probes; only 1-2 requests go to recovering provider | Could add jitter to open_duration if many instances exist |

---

## Sources

- Existing codebase: `src/proxy/server.rs`, `src/proxy/handlers.rs`, `src/proxy/retry.rs`, `src/router/selector.rs`, `src/proxy/stream.rs` -- **HIGH confidence** (direct code inspection)
- [Circuit Breaker Pattern - Microsoft Azure Architecture Center](https://learn.microsoft.com/en-us/azure/architecture/patterns/circuit-breaker) -- **HIGH confidence** (canonical reference for the pattern)
- [Martin Fowler - Circuit Breaker](https://martinfowler.com/bliki/CircuitBreaker.html) -- **HIGH confidence** (original pattern description)
- [DashMap documentation](https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html) -- **HIGH confidence** (official crate docs)
- [Resilience Design Patterns: Retry, Fallback, Timeout, Circuit Breaker](https://www.codecentric.de/en/knowledge-hub/blog/resilience-design-patterns-retry-fallback-timeout-circuit-breaker) -- **MEDIUM confidence** (architectural guidance on pattern composition)
- [Linkerd Circuit Breaking](https://linkerd.io/2-edge/reference/circuit-breaking/) -- **MEDIUM confidence** (per-endpoint circuit breaking in a proxy context)
- [circuitbreaker-rs crate](https://docs.rs/circuitbreaker-rs) -- **LOW confidence** (evaluated but not recommended; rolling our own is simpler for this use case and avoids an unnecessary dependency)
- [tower-circuitbreaker crate](https://lib.rs/crates/tower-circuitbreaker) -- **LOW confidence** (evaluated but Tower layer is wrong abstraction for this use case, as explained above)
