# Technology Stack: Per-Provider Circuit Breaker and Enhanced /health

**Project:** arbstr - Per-provider circuit breaker with 3-state machine and enhanced /health
**Researched:** 2026-02-16
**Overall confidence:** HIGH

## Scope

This research covers the stack additions/changes needed for:
1. Per-provider circuit breaker state machine (Closed/Open/HalfOpen)
2. Configurable failure threshold and recovery timeout
3. Integration with existing retry/fallback and provider selection
4. Enhanced /health endpoint exposing per-provider circuit breaker state and database status
5. DashMap for concurrent per-provider state access

## Existing Stack (No Changes Needed)

| Technology | Version (in Cargo.toml) | Purpose for This Milestone | Status |
|------------|------------------------|---------------------------|--------|
| tokio | 1 (full) | `tokio::time::Instant` for tracking when circuit opened, `tokio::time::advance()` for deterministic tests | Keep as-is |
| axum | 0.7 | Existing /health route, `State(AppState)` extraction | Keep as-is |
| serde / serde_json | 1.x | Serialize health response with provider circuit states | Keep as-is |
| tracing | 0.1 | Log circuit breaker state transitions at info level | Keep as-is |
| reqwest | 0.12 | Existing HTTP client for provider calls (no changes) | Keep as-is |

## New Dependencies Required

### dashmap = "6"

| Attribute | Value |
|-----------|-------|
| Crate | [dashmap](https://crates.io/crates/dashmap) |
| Version | 6.x (current stable) |
| Downloads | 170M+ total |
| Purpose | Concurrent HashMap with per-shard locking for circuit breaker registry |
| Transitive deps | `hashbrown` (already in dependency tree via other crates) |

**Why DashMap is the right choice for this project:**

The circuit breaker registry maps provider names to per-provider state machines. Multiple concurrent requests may simultaneously:
- Check if different providers are available (read)
- Record success/failure for different providers (write)
- Query all provider states for /health (read)

DashMap provides per-shard locking, meaning operations on different providers do not contend with each other. This is the key differentiator from `RwLock<HashMap>`.

**Confidence:** HIGH -- DashMap is the standard concurrent map in the Rust ecosystem. [Official docs](https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html) confirm per-shard locking semantics.

### Why DashMap Over Alternatives

| Alternative | Why Not |
|-------------|---------|
| `std::sync::RwLock<HashMap>` | Any write (recording failure for provider A) takes a write lock that blocks ALL reads (checking availability of provider B). Under concurrent proxy load, this serializes circuit breaker operations across all providers. |
| `tokio::sync::RwLock<HashMap>` | Async lock adds overhead for operations that complete in nanoseconds (state machine transitions have no I/O). Also has the same global contention issue as std RwLock. Circuit breaker state should never be held across `.await`. |
| `HashMap<String, Arc<Mutex<ProviderCB>>>` | Two lock acquisitions per operation (read outer HashMap + lock inner Mutex). The outer map itself needs synchronization for reads. More complex than DashMap for no benefit. |
| `parking_lot::RwLock<HashMap>` | Same global contention issue as std. Better performance under contention than std, but DashMap eliminates contention entirely via sharding. |
| Lock-free atomics (CAS loops) | Circuit breaker transitions involve multiple fields (state + counter + timestamp) that must update atomically. Packing these into atomic integers is error-prone and hard to test. DashMap gives the same effective concurrency with readable code. |

**Note on the STACK.md from the previous research round:** The prior research recommended `std::sync::RwLock<HashMap>` and stated DashMap was unnecessary for <10 providers. After deeper analysis of the actual access patterns -- particularly concurrent reads during `filter_available()` overlapping with writes during `record_failure()` -- DashMap is the better choice because it eliminates the read/write contention entirely. The cost is one dependency (with a transitive dep already in tree), the benefit is zero contention between operations on different providers.

## Recommended Implementation Stack

### Time Tracking: `tokio::time::Instant`

**Why `tokio::time::Instant` and NOT `std::time::Instant`:**

The circuit breaker uses `Instant` to track when the circuit transitioned to Open state, then compares against `Instant::now()` to determine if `open_duration` has elapsed. For testability:

- `tokio::time::Instant` responds to `tokio::time::pause()` and `tokio::time::advance()` in tests
- `std::time::Instant` does NOT respond to tokio's virtual time controls
- The existing codebase uses `tokio::test(start_paused = true)` for time-dependent retry tests (retry.rs lines 302, 334, 366, 406, 476)
- Using `tokio::time::Instant` makes circuit breaker timeout tests deterministic (no wall-clock waits)

```rust
use tokio::time::Instant;

pub struct ProviderCircuitBreaker {
    // ...
    opened_at: Option<Instant>,
    last_state_change: Instant,
}
```

**Confidence:** HIGH -- verified by existing pattern in retry.rs tests.

### Serialization: Existing serde + serde_json

The enhanced /health endpoint returns a JSON object with per-provider circuit state. Use `serde_json::json!()` macro (existing pattern) rather than typed response structs:

```rust
serde_json::json!({
    "status": overall_status,
    "service": "arbstr",
    "components": {
        "database": { "status": db_status },
        "providers": provider_health_array
    }
})
```

**Why `json!()` instead of `#[derive(Serialize)]`:** The health response is a diagnostic endpoint, not a data API. Its shape may evolve frequently (add fields for new components). The `json!()` macro avoids maintaining a typed struct that changes with every new health indicator. The existing `/providers` and `/health` endpoints both use `json!()` for this reason.

### Logging: Existing tracing

Log circuit breaker state transitions at `info` level for observability:

```rust
tracing::info!(
    provider = %name,
    from = ?old_state,
    to = ?new_state,
    failure_count = breaker.failure_count,
    "Circuit breaker state transition"
);
```

No new tracing features or dependencies needed.

## Circuit Breaker Configuration

### Hardcoded Constants (Not in config.toml)

```rust
/// Number of consecutive failures before opening the circuit.
const DEFAULT_FAILURE_THRESHOLD: u32 = 5;

/// How long the circuit stays open before transitioning to HalfOpen.
const DEFAULT_OPEN_DURATION: Duration = Duration::from_secs(30);

/// Number of successful probes needed in HalfOpen to close the circuit.
const DEFAULT_HALF_OPEN_SUCCESS_THRESHOLD: u32 = 2;
```

**Why hardcoded, not configurable:**
- The circuit breaker is new. Getting thresholds right requires operational experience.
- Premature config surface area forces users to make decisions they cannot yet make.
- Constants at the top of the file are easy to find and change.
- If users request tuning, promote to `config.toml` in a future milestone.
- The `CircuitBreakerConfig` struct supports runtime values, making promotion trivial.

**Why these specific defaults:**
- **5 failures:** Conservative enough to avoid tripping on transient errors (single bad request, brief network blip). The retry system already handles 3 transient failures per request. 5 consecutive failures across multiple requests signals a real problem.
- **30 seconds:** Long enough for a provider to recover from a restart or brief overload. Short enough that users do not wait long before the proxy retries. Matches common circuit breaker defaults (Resilience4j default is 60s, Hystrix was 5s; 30s is a reasonable middle ground for AI provider APIs).
- **2 successes in HalfOpen:** Prevents premature closure on a lucky single request. Two successes provide modest confidence the provider is actually recovered.

## Files Changed

| File | Change | Why |
|------|--------|-----|
| `Cargo.toml` | Add `dashmap = "6"` | Concurrent per-provider state map |
| `src/proxy/circuit_breaker.rs` | **NEW** | Circuit breaker state machine, registry, health info types |
| `src/proxy/mod.rs` | Add `pub mod circuit_breaker;` | Module registration |
| `src/proxy/server.rs` | Add `circuit_breakers` to AppState, initialize in `run_server()` | Shared state |
| `src/proxy/handlers.rs` | Filter candidates, record outcomes, enhance /health | Integration |
| `src/error.rs` | Optional: add `Error::CircuitOpen` variant | Clearer error messages |

## Files NOT Changed

| File | Why Not |
|------|---------|
| `src/config.rs` | Circuit breaker params are hardcoded constants, not user-configurable |
| `src/router/selector.rs` | Router stays pure (model/cost/policy). Circuit breaker filtering at handler level. |
| `src/proxy/retry.rs` | Retry logic unchanged -- circuit breaker operates before retry starts |
| `src/proxy/stream.rs` | SSE streaming logic unaffected |
| `src/storage/` | No schema changes, no new queries |
| `migrations/` | No database changes |

## Cargo.toml Change

```toml
[dependencies]
# ... existing deps unchanged ...

# Concurrency
dashmap = "6"
```

**Net dependency change: +1 crate** (dashmap). Transitive dep `hashbrown` is already in the tree.

## Version Verification

| Crate | Version (Cargo.toml) | Latest Stable | Action | Confidence |
|-------|---------------------|---------------|--------|------------|
| tokio | 1 (full) | 1.43+ | Keep -- `tokio::time::Instant` available since 1.0 | HIGH |
| axum | 0.7 | 0.8.x | Keep at 0.7 -- no new features needed | HIGH |
| dashmap | (new) | 6.x | Add at 6 -- current stable | HIGH |
| serde | 1.x | 1.x | Keep -- stable | HIGH |
| tracing | 0.1 | 0.1.x | Keep -- stable | HIGH |

## Testing Strategy

### Time-Dependent Tests

Use `tokio::test(start_paused = true)` with `tokio::time::advance()` for deterministic circuit breaker timeout tests. This matches the existing pattern in `retry.rs`.

```rust
#[tokio::test(start_paused = true)]
async fn test_open_transitions_to_half_open_after_timeout() {
    let config = CircuitBreakerConfig {
        failure_threshold: 1,
        open_duration: Duration::from_secs(30),
        ..Default::default()
    };
    let mut cb = ProviderCircuitBreaker::new();
    cb.record_failure(&config);
    assert!(!cb.is_available(&config));

    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(cb.is_available(&config));
}
```

### Unit Testable (No Server Needed)

- State transitions: Closed -> Open after N failures
- Counter reset on success
- HalfOpen -> Closed on probe success
- HalfOpen -> Open on probe failure
- Failure threshold boundary behavior
- Registry filter_available() removes Open providers
- Registry provider_states() returns correct snapshots

### Integration Testable

- Handler skips Open providers and selects next cheapest
- All providers Open returns 503 (not 400)
- /health reports correct circuit state per provider
- /health shows "degraded" when some providers are Open
- x-arbstr-circuit-state header present when filtering occurs

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Concurrent map | `DashMap` | `std::sync::RwLock<HashMap>` | Write contention blocks all reads. DashMap's sharding eliminates cross-provider contention. |
| Concurrent map | `DashMap` | `tokio::sync::RwLock<HashMap>` | Async locks unnecessary for nanosecond operations. Same contention issue as std. |
| Circuit breaker lib | Custom ~150 lines | `failsafe` 1.3.0 | Call-wrapping API conflicts with arbstr's existing retry architecture. |
| Circuit breaker lib | Custom ~150 lines | `tower-circuitbreaker` 0.1.0 | Requires Tower Service trait. 823 downloads, immature. |
| Circuit breaker lib | Custom ~150 lines | `recloser` | Failure-rate model (ring buffer), not consecutive-failure counting. |
| Time tracking | `tokio::time::Instant` | `std::time::Instant` | Does not respond to `tokio::time::pause()` for deterministic tests. |
| Config | Hardcoded constants | `config.toml` options | Premature configurability. Lock to defaults, promote later if needed. |
| State storage | In-memory (AppState) | SQLite table | Circuit breaker state is ephemeral. Should reset on restart. Persistence adds complexity for negative value. |

## Sources

### Primary (HIGH confidence)
- Local codebase analysis: `Cargo.toml`, `src/proxy/server.rs` (AppState), `src/proxy/handlers.rs` (health handler, send_to_provider), `src/proxy/retry.rs` (retry pattern, start_paused tests), `src/router/selector.rs` (select_candidates) -- all read and verified
- [DashMap documentation](https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html) -- per-shard locking confirmed
- [DashMap on crates.io](https://crates.io/crates/dashmap) -- 170M+ downloads, version 6.x stable
- [tokio::time::Instant docs](https://docs.rs/tokio/latest/tokio/time/struct.Instant.html) -- virtual time control for testing confirmed

### Secondary (MEDIUM confidence)
- [failsafe crate docs](https://docs.rs/failsafe/latest/failsafe/) -- call-wrapping API confirmed, architectural mismatch
- [tower-circuitbreaker docs](https://docs.rs/tower-circuitbreaker/latest/tower_circuitbreaker/) -- Tower Service middleware confirmed
- [Resilience4j Circuit Breaker Configuration](https://resilience4j.readme.io/docs/circuitbreaker) -- default values reference (60s wait, sliding window)
- [Circuit Breaker Pattern - Microsoft Azure](https://learn.microsoft.com/en-us/azure/architecture/patterns/circuit-breaker) -- canonical pattern description, lazy vs proactive probing
