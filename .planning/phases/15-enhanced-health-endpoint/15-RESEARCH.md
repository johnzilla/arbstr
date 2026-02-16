# Phase 15: Enhanced Health Endpoint - Research

**Researched:** 2026-02-16
**Domain:** HTTP health check endpoint with circuit breaker state aggregation
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- Providers keyed by name as an object: `{"providers": {"alpha": {"state": "closed", "failure_count": 0}}}`
- Each provider entry includes only `state` and `failure_count` -- minimal, matches success criteria
- Top-level has only `status` field plus `providers` object -- no aggregate counts or uptime
- Circuit state values are lowercase strings: `"closed"`, `"open"`, `"half_open"`
- HTTP 200 for `ok` and `degraded`, HTTP 503 only for `unhealthy` (all circuits open)
- Half-open providers count as degraded for top-level status calculation
- Zero configured providers returns `"ok"` with empty providers object -- server is running fine
- No timestamps in the response -- state and failure_count are sufficient
- Replace existing `/health` response in-place -- clean break, new response shape entirely
- No versioning or separate endpoint -- this is a local proxy with no external consumers
- Open endpoint, no auth -- consistent with current behavior
- Content-Type: application/json, consistent with all other endpoints

### Claude's Discretion
- Internal implementation approach (how to query circuit breaker registry)
- Response serialization pattern (serde structs vs manual JSON)
- Test structure and coverage approach

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| HLT-01 | GET /health returns per-provider circuit state (closed/open/half_open) and failure count | Registry already has `state()` and `failure_count()` accessors; provider names available from `AppState.router.providers()` or a new `all_states()` method on the registry |
| HLT-02 | Top-level status degrades: ok (all closed) -> degraded (some open) -> unhealthy (all open) | Status logic is pure computation from the set of circuit states; half-open counts as degraded per user decision |
</phase_requirements>

## Summary

This phase replaces the current trivial `/health` endpoint (which returns `{"status": "ok", "service": "arbstr"}`) with an enhanced version that reports per-provider circuit breaker state and a computed top-level health status. The implementation is straightforward because all necessary infrastructure already exists:

1. **Circuit breaker state is already queryable**: `CircuitBreakerRegistry` has read-only accessors `state(name)` and `failure_count(name)` specifically annotated "for Phase 15 health endpoint" (added in Phase 13).
2. **Provider names are available** from `AppState.router.providers()` which returns `&[ProviderConfig]`.
3. **The handler signature changes** from `health()` (no state) to `health(State(state): State<AppState>)` to access the circuit breaker registry and provider list.

The main implementation decisions are: (a) how to iterate over all providers to build the response, (b) whether to use serde structs or `serde_json::json!` for the response, and (c) how to compute the top-level status from the set of circuit states.

**Primary recommendation:** Use a new `all_states()` method on `CircuitBreakerRegistry` that returns a `Vec<(String, CircuitState, u32)>` (name, state, failure_count). Use serde structs for the response to get compile-time type safety. Compute top-level status with simple match logic on the count of open/half-open circuits.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| axum | 0.7 | HTTP handler with `State` extractor | Already used for all endpoints |
| serde + serde_json | 1.x | JSON serialization | Already used project-wide |
| dashmap | 6.1.0 | Concurrent map backing circuit breaker registry | Already in use; `iter()` method needed |

### Supporting
No new libraries needed. Everything required is already in the dependency tree.

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Serde structs for response | `serde_json::json!` macro | json! is less code but no compile-time guarantees; structs match project's typed pattern (StatsResponse, LogsResponse) |
| New `all_states()` on registry | Query per-provider via existing `state()` + `failure_count()` | Per-provider query acquires lock per call; `all_states()` can iterate DashMap once and collect |

## Architecture Patterns

### Recommended Changes

```
src/proxy/
├── handlers.rs         # health() handler updated: takes State, returns HealthResponse
├── circuit_breaker.rs  # Add all_states() to CircuitBreakerRegistry
tests/
├── health.rs           # NEW: integration tests for /health endpoint
```

### Pattern 1: New `all_states()` on CircuitBreakerRegistry

**What:** A method that iterates over the DashMap and returns all provider states in one pass.
**When to use:** When the health endpoint needs the state of all providers at once.
**Why:** Avoids N separate lock acquisitions (one per `state()` + one per `failure_count()` = 2N locks). A single `iter()` on DashMap acquires each shard lock once, reads all entries in that shard, then moves on.

```rust
/// Snapshot of a single provider's circuit breaker state.
pub struct CircuitSnapshot {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
}

impl CircuitBreakerRegistry {
    /// Return a snapshot of all provider circuit states.
    ///
    /// Uses DashMap::iter() which acquires per-shard locks (not a global lock).
    /// Each shard is locked only while its entries are being read.
    pub fn all_states(&self) -> Vec<CircuitSnapshot> {
        self.breakers
            .iter()
            .map(|entry| {
                let inner = entry.value().inner.lock().unwrap();
                CircuitSnapshot {
                    name: entry.key().clone(),
                    state: inner.state,
                    failure_count: inner.failure_count,
                }
            })
            .collect()
    }
}
```

**DashMap iteration note:** DashMap 6's `iter()` does NOT hold all shard locks simultaneously. It acquires each shard's read lock one at a time, reads entries, releases, then moves to the next shard. This is safe for health endpoint usage and introduces no cross-shard contention. However, it does hold both the DashMap shard read lock AND the `std::sync::Mutex` on the inner state simultaneously for each entry. Since the inner lock is never held across `.await` points and the health handler is synchronous-in-lock, this is fine.

### Pattern 2: Serde Response Struct

**What:** Typed response structs with `#[derive(Serialize)]` for the health response.
**When to use:** Following project convention (see `StatsResponse`, `LogEntry`).
**Example:**

```rust
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub providers: HashMap<String, ProviderHealth>,
}

#[derive(Debug, Serialize)]
pub struct ProviderHealth {
    pub state: String,
    pub failure_count: u32,
}
```

**Recommendation:** Use serde structs. The project consistently uses typed structs for response shapes (StatsResponse, LogEntry, etc.). The `json!` macro is only used in simple leaf handlers like `list_models` and the current `health()`.

### Pattern 3: Status Computation

**What:** Derive top-level status from the set of circuit states.
**Logic:**
- If providers is empty: `"ok"` (no providers configured, server is running)
- If ALL circuits are open: `"unhealthy"` -> HTTP 503
- If ANY circuit is open OR half-open: `"degraded"` -> HTTP 200
- Otherwise (all closed): `"ok"` -> HTTP 200

```rust
fn compute_status(states: &[CircuitSnapshot]) -> (&'static str, StatusCode) {
    if states.is_empty() {
        return ("ok", StatusCode::OK);
    }

    let open_count = states.iter().filter(|s| s.state == CircuitState::Open).count();
    let half_open_count = states.iter().filter(|s| s.state == CircuitState::HalfOpen).count();

    if open_count == states.len() {
        ("unhealthy", StatusCode::SERVICE_UNAVAILABLE)
    } else if open_count > 0 || half_open_count > 0 {
        ("degraded", StatusCode::OK)
    } else {
        ("ok", StatusCode::OK)
    }
}
```

**Note on "all open" semantics:** The decision says "unhealthy when all circuits open." A half-open circuit is NOT open for this calculation -- half-open means recovery is being attempted, which is degraded but not fully down. Only `CircuitState::Open` counts toward the "all open" check.

### Pattern 4: CircuitState Display

**What:** Convert `CircuitState` enum to lowercase string for JSON response.
**Approach:** Implement a `as_str()` method or `Display` trait on `CircuitState`.

```rust
impl CircuitState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CircuitState::Closed => "closed",
            CircuitState::Open => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }
}
```

Using `as_str()` is preferred over `Display` because the lowercase/underscore format is JSON-specific, not a general display format.

### Anti-Patterns to Avoid
- **Holding DashMap entry ref across handler boundary:** Never store `DashMap::get()` return values. Collect all data inside the iteration, then drop the refs.
- **Acquiring inner Mutex inside async context:** The `all_states()` method is synchronous and should remain so. The health handler itself is `async` but the lock acquisition is in a sync closure inside `iter().map()`.
- **Returning serde_json::Value from handler:** Use typed structs instead. `Json<HealthResponse>` gives compile-time serialization guarantee.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON serialization | Manual string formatting | `serde::Serialize` derive + `axum::Json` | Escaping, field ordering, null handling |
| Concurrent map iteration | Custom lock orchestration | `DashMap::iter()` | Per-shard locking already implemented correctly |

**Key insight:** This phase is pure plumbing -- connecting existing data (circuit breaker state) to a new response format. No new algorithms, no new data structures.

## Common Pitfalls

### Pitfall 1: Lock Ordering Between DashMap and Inner Mutex
**What goes wrong:** Deadlock if DashMap shard lock and inner `std::sync::Mutex` are acquired in inconsistent order.
**Why it happens:** `all_states()` must acquire both: DashMap shard lock (via `iter()`) and inner Mutex (via `entry.value().inner.lock()`).
**How to avoid:** Always acquire DashMap shard lock first (outer), then inner Mutex. This is the natural order in `iter().map(|entry| entry.value().inner.lock())`. All existing methods (`record_success`, `record_failure`, `state()`, `failure_count()`) follow this same order: DashMap `.get()` then `.inner.lock()`.
**Warning signs:** Any code path that acquires inner Mutex first then calls DashMap methods.

### Pitfall 2: Half-Open Status Classification
**What goes wrong:** Treating half-open as "open" for the "all open = unhealthy" check, causing false 503s during recovery.
**Why it happens:** Ambiguity between "any non-closed = unhealthy" vs the three-tier status decision.
**How to avoid:** The decision explicitly states: half-open counts as *degraded*, not unhealthy. Only `CircuitState::Open` (strict) counts for the "all open" threshold.
**Warning signs:** Test that has one half-open + one open, expecting 503.

### Pitfall 3: Handler Signature Change Breaks Compilation
**What goes wrong:** Current `health()` takes no arguments. Changing it to `health(State(state): State<AppState>)` requires updating the route registration.
**Why it happens:** axum's type system enforces handler extractors at compile time.
**How to avoid:** The route in `server.rs` is `.route("/health", get(handlers::health))`. Since `health()` returns `impl IntoResponse`, adding `State(state)` is backward compatible with the route setup -- axum resolves extractors by type, and `State` is already registered via `.with_state(state)`.
**Warning signs:** Compiler error about missing `FromRequestParts` implementation.

### Pitfall 4: DashMap Iteration Order
**What goes wrong:** DashMap iteration order is non-deterministic. Tests that assert specific provider ordering in the response will be flaky.
**Why it happens:** DashMap is hash-based with per-shard iteration.
**How to avoid:** Use a `HashMap` (or `BTreeMap` for deterministic ordering) in the response struct. For tests, assert on individual provider entries by key, not on ordering.
**Warning signs:** Test failures that are intermittent based on hash seed.

## Code Examples

### Current Health Handler (to be replaced)
```rust
// Source: src/proxy/handlers.rs, lines 1119-1125
/// Handle GET /health
pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "arbstr"
    }))
}
```

### Existing Circuit Breaker Read Accessors (Phase 13 preparation)
```rust
// Source: src/proxy/circuit_breaker.rs, lines 436-454
impl CircuitBreakerRegistry {
    /// Read-only accessor for circuit state (for Phase 15 health endpoint).
    pub fn state(&self, provider_name: &str) -> Option<CircuitState> {
        self.breakers
            .get(provider_name)
            .map(|entry| entry.value().inner.lock().unwrap().state)
    }

    /// Read-only accessor for current failure count (for Phase 15).
    pub fn failure_count(&self, provider_name: &str) -> Option<u32> {
        self.breakers
            .get(provider_name)
            .map(|entry| entry.value().inner.lock().unwrap().failure_count)
    }
}
```

### Provider Name Iteration (from AppState)
```rust
// Source: src/proxy/server.rs + src/router/selector.rs
// AppState.router is Arc<ProviderRouter>
// ProviderRouter::providers() returns &[ProviderConfig]
// ProviderConfig.name is the provider identifier
let provider_names: Vec<&str> = state.router.providers().iter().map(|p| p.name.as_str()).collect();
```

### Integration Test Pattern (from circuit_integration.rs)
```rust
// Source: tests/circuit_integration.rs
// Test setup pattern: create providers, build app, use oneshot
fn setup_circuit_test_app(
    providers: Vec<ProviderConfig>,
) -> (axum::Router, Arc<CircuitBreakerRegistry>) {
    // ... builds AppState with registry, returns (router, registry)
}

// Test execution: build request, call oneshot, check response
let request = Request::get("/health")
    .body(Body::empty())
    .unwrap();
let response = app.oneshot(request).await.unwrap();
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Simple `{"status": "ok"}` health | Per-provider circuit state reporting | This phase | Operators get per-provider visibility |

**Deprecated/outdated:**
- The `"service": "arbstr"` field in the current health response is being dropped (not part of new schema)

## Open Questions

1. **DashMap iteration vs per-provider query approach**
   - What we know: Both work. `all_states()` is one DashMap pass with N inner locks. Per-provider query via existing `state()` + `failure_count()` is 2N DashMap lookups + 2N inner locks.
   - What's unclear: Whether the difference matters at all for a handful of providers.
   - Recommendation: Use `all_states()` for correctness (atomic snapshot per-provider: state and failure_count read under same lock) and cleanliness. The per-provider approach could read state=Closed then failure_count=3 (after a race) which is internally inconsistent.

2. **Serde structs vs `serde_json::json!` macro**
   - What we know: Both work. The project uses structs for complex responses (stats, logs) and json! for simple ones (models, current health, providers).
   - Recommendation: Use serde structs. The response shape is well-defined and the struct provides type safety. The `HashMap<String, ProviderHealth>` serializes naturally as a JSON object keyed by provider name.

## Sources

### Primary (HIGH confidence)
- Codebase: `src/proxy/circuit_breaker.rs` -- CircuitBreakerRegistry API, state/failure_count accessors, DashMap usage
- Codebase: `src/proxy/handlers.rs` -- Current health handler, handler patterns
- Codebase: `src/proxy/server.rs` -- AppState structure, route registration, create_router
- Codebase: `tests/circuit_integration.rs` -- Integration test patterns with setup_circuit_test_app
- Codebase: `src/router/selector.rs` -- Router::providers() accessor

### Secondary (MEDIUM confidence)
- DashMap 6.1.0 crate -- `iter()` method provides per-shard locking, not global lock. Verified by crate version in Cargo.lock and known DashMap API.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - no new dependencies, all existing crate APIs verified in source
- Architecture: HIGH - straightforward handler change, existing patterns well-established
- Pitfalls: HIGH - lock ordering verified against existing code, DashMap iteration semantics well-known

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable domain, no external dependencies)
