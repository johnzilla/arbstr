# Phase 13: Circuit Breaker State Machine - Research

**Researched:** 2026-02-16
**Domain:** Per-provider 3-state circuit breaker state machine (Closed/Open/Half-Open) with queue-and-wait half-open probing
**Confidence:** HIGH

## Summary

This phase builds the circuit breaker state machine as an independently testable module. Each provider gets its own circuit breaker that tracks consecutive 5xx/timeout failures and automatically recovers via the standard Closed -> Open -> Half-Open -> Closed cycle. The state machine is the foundation -- Phase 14 (routing integration) and Phase 15 (health endpoint) consume it.

The primary technical challenges are: (1) implementing queue-and-wait semantics in Half-Open state where one probe request runs while others wait for its result, (2) combining synchronous state transitions (std::sync::Mutex) with asynchronous waiting (tokio primitives), and (3) ensuring deterministic testability using tokio::time::Instant with start_paused.

**Primary recommendation:** Build `src/proxy/circuit_breaker.rs` with a `CircuitBreakerRegistry` backed by DashMap, where each entry holds a `std::sync::Mutex<CircuitBreakerInner>` for the state machine plus a `tokio::sync::watch` channel for probe-result signaling. Use `tokio::time::Instant` for all timestamps to enable deterministic testing.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

#### Failure classification
- Only HTTP 5xx responses and request timeouts count as circuit-tripping failures
- All 4xx responses (including 429 rate limits) are ignored by the circuit breaker
- Network-level errors (connection refused, DNS failure, TLS errors) do NOT trip the circuit -- only timeout and 5xx
- Single request timeout model (one duration for the entire request, not separate connect/response timeouts)
- Streaming: only the initial HTTP response status matters -- if 2xx is received and streaming begins, it counts as success even if the stream fails mid-way

#### Transition logging
- Log state transitions using tracing (not individual failure increments)
- WARN level when circuit opens (something is wrong)
- INFO level when circuit closes or enters half-open (recovery)
- Include reason in log messages: failure count, last error type, provider name (e.g., "provider-alpha circuit OPENED: 3 consecutive 5xx")

#### Half-open behavior
- Circuit breaker returns a typed CircuitOpen error when requests hit an open circuit -- callers decide what to do
- Single-permit half-open: exactly one probe request allowed
- Queue-and-wait during probe: if probe is in-flight, subsequent requests for that provider wait for the probe result
- All waiting requests wait (no queue limit)
- If probe succeeds: circuit closes, waiting requests proceed
- If probe fails: circuit reopens with fresh 30s timer, all waiting requests receive CircuitOpen error immediately

#### Error context
- Store the last error that caused the most recent state change (error type/message)
- Track timestamps for state transitions: opened_at, last failure time, last success time
- Track cumulative trip count (total times this circuit has tripped) -- signals chronically unhealthy providers
- No manual reset -- circuit breaker is purely automatic

### Claude's Discretion
- Internal data structure layout within the Mutex guard
- Exact CircuitOpen error type design
- Queue-and-wait implementation mechanism (tokio::watch, Notify, etc.)
- Test structure and mock patterns

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| CB-01 | Each provider has an independent circuit breaker with 3 states (Closed, Open, Half-Open) | DashMap registry keyed by provider name; CircuitState enum; per-provider Mutex<CircuitBreakerInner>. Patterns verified in existing codebase (providers identified by name string throughout). |
| CB-02 | Circuit opens after 3 consecutive request failures (5xx/timeout only, not 4xx) | Hardcoded FAILURE_THRESHOLD=3 constant. Failure classification via enum FailureKind { ServerError(u16), Timeout }. Consecutive counter resets on success (CB-03). |
| CB-03 | Successful request resets the consecutive failure counter to zero | record_success() sets failure_count=0 in Closed state; transitions HalfOpen->Closed on probe success (CB-06). Both paths reset the counter. |
| CB-04 | After 30s in Open state, circuit transitions to Half-Open for probe | Lazy transition: is_available() checks `tokio::time::Instant::now() >= opened_at + OPEN_DURATION`. No background timer. Deterministic testing via start_paused + advance(). |
| CB-05 | Half-Open allows exactly one probe request (single-permit model) | probe_in_flight boolean flag in state. First caller gets permit, subsequent callers wait on tokio::sync::watch channel for probe result. |
| CB-06 | Probe success closes circuit; probe failure reopens with timer reset | record_probe_success() transitions to Closed, broadcasts Ok via watch channel. record_probe_failure() transitions to Open with fresh opened_at, broadcasts Err via watch channel. |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| dashmap | 6.x (stable) | Per-provider concurrent map for circuit breaker registry | 170M+ downloads, per-shard locking eliminates cross-provider contention. Already researched and verified for this project. |
| tokio | 1 (full, existing) | `tokio::time::Instant` for timestamps, `tokio::sync::watch` for probe signaling, `start_paused`/`advance` for tests | Already in Cargo.toml. Instant responds to virtual time in tests. |
| tracing | 0.1 (existing) | State transition logging at WARN/INFO levels | Already in Cargo.toml. |
| thiserror | 1 (existing) | CircuitOpen error type | Already in Cargo.toml. Matches existing Error enum pattern. |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| std::sync::Mutex | stdlib | Guards per-provider state machine fields | All state transitions -- no .await inside lock. Matches existing AttemptRecord pattern in retry.rs. |
| tokio::sync::watch | 1 (in tokio) | Broadcast probe result to waiting requests | Half-Open queue-and-wait: probe completes -> watch::Sender::send() -> all watch::Receiver::changed().await wake up. |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| dashmap | RwLock<HashMap> | Write contention blocks all reads. DashMap sharding eliminates cross-provider contention. |
| std::sync::Mutex | tokio::sync::Mutex | Async lock unnecessary for nanosecond operations (no .await inside lock). Adds overhead. |
| tokio::sync::watch | tokio::sync::Notify + separate storage | Notify doesn't carry data. Watch combines notification + result storage in one primitive. Cleaner API. |
| tokio::sync::watch | tokio::sync::broadcast | Broadcast requires receivers to be created before send. Watch stores latest value, so receivers created after send still see it. Watch is correct for "check latest result" semantics. |

**Installation:**
```toml
# Add to [dependencies] in Cargo.toml
dashmap = "6"
```

## Architecture Patterns

### Recommended Module Structure
```
src/proxy/
  circuit_breaker.rs    # NEW: CircuitState, CircuitBreakerInner,
                        #       ProviderCircuitBreaker, CircuitBreakerRegistry,
                        #       CircuitOpenError, ProbeResult
```

Single file is appropriate. The module is approximately 250-350 lines (state machine + registry + types + tests). If it grows beyond 500 lines, split into `circuit_breaker/mod.rs` + `circuit_breaker/state.rs` + `circuit_breaker/registry.rs`.

### Pattern 1: Two-Layer Locking (DashMap + Mutex)

**What:** DashMap provides per-shard concurrent access to the registry. Each entry holds a `ProviderCircuitBreaker` that contains a `Mutex<CircuitBreakerInner>` for the state machine fields plus a watch channel for probe signaling.

**When to use:** All registry operations (check availability, record outcomes).

**Why two layers:** DashMap gives concurrent access to different providers. The inner Mutex serializes state transitions within a single provider. This is required because state transitions involve multiple field updates (state + counter + timestamp + watch channel) that must be atomic.

**Example:**
```rust
use dashmap::DashMap;
use std::sync::Mutex;
use tokio::sync::watch;

pub struct CircuitBreakerRegistry {
    breakers: DashMap<String, ProviderCircuitBreaker>,
}

pub struct ProviderCircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
    /// Watch channel for probe result signaling in Half-Open state.
    /// Sender held inside the Mutex guard during state updates.
    /// Receivers cloned by waiting tasks.
    probe_watch: watch::Sender<ProbeResult>,
}

struct CircuitBreakerInner {
    state: CircuitState,
    failure_count: u32,
    opened_at: Option<tokio::time::Instant>,
    last_failure_time: Option<tokio::time::Instant>,
    last_success_time: Option<tokio::time::Instant>,
    last_error: Option<LastError>,
    trip_count: u32,
    probe_in_flight: bool,
}
```

**Source:** Codebase analysis -- existing `Arc<Mutex<Vec<AttemptRecord>>>` pattern in retry.rs (line 9), DashMap verified in official docs.

### Pattern 2: Lazy Open-to-HalfOpen Transition

**What:** The Open -> HalfOpen transition is not triggered by a background timer. Instead, it is checked lazily when `check_state()` or `try_acquire_permit()` is called. If `Instant::now() >= opened_at + OPEN_DURATION`, the state transitions to HalfOpen on the spot.

**When to use:** Every availability check. The caller asks "can I send a request to this provider?" and the state machine checks if the Open timeout has expired.

**Why lazy:**
- No background tasks to manage or cancel
- No risk of timer drift or missed transitions
- Simpler to test (control time via `tokio::time::advance()`)
- If no requests come in while Open, no wasted probe
- Matches prior decision from additional_context

**Example:**
```rust
impl CircuitBreakerInner {
    /// Check if a request should be allowed through.
    /// Returns Closed (proceed), NeedProbe (caller is the probe),
    /// WaitForProbe (another probe in flight), or Open (blocked).
    fn check_state(&mut self) -> CheckResult {
        match self.state {
            CircuitState::Closed => CheckResult::Allowed,
            CircuitState::Open => {
                if let Some(opened_at) = self.opened_at {
                    if tokio::time::Instant::now().duration_since(opened_at) >= OPEN_DURATION {
                        self.state = CircuitState::HalfOpen;
                        self.probe_in_flight = false;
                        tracing::info!(/* ... */ "circuit entering Half-Open");
                        // Fall through to HalfOpen handling below
                        self.try_acquire_probe()
                    } else {
                        CheckResult::Rejected
                    }
                } else {
                    CheckResult::Rejected
                }
            }
            CircuitState::HalfOpen => self.try_acquire_probe(),
        }
    }

    fn try_acquire_probe(&mut self) -> CheckResult {
        if !self.probe_in_flight {
            self.probe_in_flight = true;
            CheckResult::ProbePermit
        } else {
            CheckResult::WaitForProbe
        }
    }
}
```

### Pattern 3: Watch Channel for Probe Result Broadcasting

**What:** When a probe is in flight during Half-Open state, subsequent requests that arrive for the same provider need to wait for the probe result rather than being rejected immediately. A `tokio::sync::watch` channel is used to broadcast the probe result.

**When to use:** Half-Open state queue-and-wait.

**How it works:**
1. First request in Half-Open acquires the probe permit (`probe_in_flight = true`)
2. Subsequent requests see `probe_in_flight == true`, clone a `watch::Receiver`, release the Mutex, then `.changed().await`
3. Probe completes (success or failure) -> updates state -> `watch::Sender::send(result)`
4. All waiting receivers wake up and read the result
5. On probe success: state is now Closed, waiters proceed with their requests
6. On probe failure: state is now Open, waiters receive CircuitOpen error

**Why watch over Notify:** `Notify` does not carry data. Waiters need to know whether the probe succeeded or failed to decide their next action. `watch` combines notification with result storage.

**Why watch over broadcast:** `broadcast` requires receivers to exist before the send. With `watch`, the latest value is stored, so even if timing is tight, receivers always see the latest result. Also, `watch` uses less memory (one stored value vs ring buffer).

**Example:**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// Initial state -- no probe has completed yet.
    Pending,
    /// Probe succeeded, circuit is now Closed.
    Success,
    /// Probe failed, circuit is now Open.
    Failed,
}

pub struct ProviderCircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
    probe_watch: watch::Sender<ProbeResult>,
}

impl CircuitBreakerRegistry {
    /// Check if a provider is available and acquire a permit if needed.
    /// Returns Ok(()) if the request should proceed, Err(CircuitOpenError)
    /// if blocked, or waits if a probe is in flight.
    pub async fn acquire_permit(&self, provider_name: &str) -> Result<(), CircuitOpenError> {
        let entry = self.breakers.get(provider_name);
        let Some(entry) = entry else {
            // Unknown provider -- allow (circuit breaker is opt-in)
            return Ok(());
        };
        let cb = entry.value();

        let check_result = {
            let mut inner = cb.inner.lock().unwrap();
            inner.check_state()
        };
        // Mutex released here -- critical: no .await while holding Mutex

        match check_result {
            CheckResult::Allowed | CheckResult::ProbePermit => Ok(()),
            CheckResult::Rejected => Err(CircuitOpenError { /* ... */ }),
            CheckResult::WaitForProbe => {
                // Clone a receiver and wait outside the lock
                let mut rx = cb.probe_watch.subscribe();
                // Wait for probe result
                let _ = rx.changed().await;
                let result = *rx.borrow();
                match result {
                    ProbeResult::Success => Ok(()),
                    ProbeResult::Failed | ProbeResult::Pending => {
                        Err(CircuitOpenError { /* ... */ })
                    }
                }
            }
        }
    }
}
```

**Critical implementation detail:** The Mutex is released BEFORE any `.await` point. The `watch::Receiver` is cloned from the `watch::Sender` while holding the DashMap shard lock (via `entry.value()`), but the actual `.changed().await` happens after all locks are released. This ensures:
- No Mutex held across await (std::sync::Mutex contract satisfied)
- No DashMap shard lock held across await (prevents blocking other providers)
- Multiple waiters can all await independently on their own receiver clones

### Pattern 4: CircuitOpen Error Type

**What:** A dedicated error type that callers receive when a circuit is open. Contains enough context for the caller to make decisions (which provider, why, when it might recover).

**Example:**
```rust
/// Error returned when a provider's circuit breaker is open.
#[derive(Debug, Clone)]
pub struct CircuitOpenError {
    /// Name of the provider whose circuit is open.
    pub provider: String,
    /// Why the circuit is open (last error that caused the transition).
    pub reason: String,
    /// How many times this circuit has tripped (cumulative).
    pub trip_count: u32,
}

impl std::fmt::Display for CircuitOpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Circuit breaker open for provider '{}': {}",
            self.provider, self.reason
        )
    }
}

impl std::error::Error for CircuitOpenError {}
```

**Design choice:** Separate struct rather than a variant of the existing `Error` enum. Rationale:
- The circuit breaker module should be self-contained (Phase 14 integration adds it to `Error` enum later)
- Callers receive a typed error with structured fields, not just a string
- The `trip_count` field helps Phase 14 decide whether to log at WARN (first trip) vs ERROR (chronic)

### Pattern 5: Structured Logging on State Transitions

**What:** Log at WARN when circuit opens, INFO when it recovers. Include provider name, failure count, and last error type.

**Example:**
```rust
fn transition_to_open(&mut self, provider_name: &str) {
    let old_state = self.state;
    self.state = CircuitState::Open;
    self.opened_at = Some(tokio::time::Instant::now());
    self.trip_count += 1;

    tracing::warn!(
        provider = %provider_name,
        failure_count = self.failure_count,
        last_error = ?self.last_error,
        trip_count = self.trip_count,
        "circuit OPENED: {} consecutive failures",
        self.failure_count,
    );
}

fn transition_to_closed(&mut self, provider_name: &str) {
    self.state = CircuitState::Closed;
    self.failure_count = 0;
    self.probe_in_flight = false;

    tracing::info!(
        provider = %provider_name,
        trip_count = self.trip_count,
        "circuit CLOSED: probe succeeded",
    );
}
```

### Anti-Patterns to Avoid

- **Holding Mutex across .await:** The inner state Mutex must NEVER be held while awaiting the probe result. Clone the watch::Receiver, drop the Mutex guard, then await. Violating this will cause deadlocks because other tasks cannot record probe results while someone holds the lock.

- **Using atomics for multi-field state:** Circuit breaker transitions involve state + counter + timestamp + probe_in_flight. Atomics cannot update all fields atomically. Use Mutex.

- **Background timer for Open->HalfOpen:** Adds complexity (task management, cancellation) for no benefit. Lazy check at request time is simpler and correct.

- **Recording failures in Open state:** When a circuit is Open, requests are rejected. Do not increment the failure counter for rejected requests -- the counter only tracks actual provider failures.

- **Re-creating watch channel on every probe cycle:** The watch::Sender should persist across probe cycles. When a new probe cycle starts (Open->HalfOpen), send `ProbeResult::Pending` to reset the channel state. Receivers from the previous cycle will see the update and can be ignored.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Concurrent per-provider map | Manual sharded HashMap | DashMap 6.x | Battle-tested, correct per-shard locking, 170M+ downloads |
| Probe result broadcast | Manual mpsc + Arc<Mutex<Result>> | tokio::sync::watch | Built-in latest-value semantics, automatic multi-receiver notification |
| Deterministic time in tests | Real-time waits with tolerance | tokio::time::Instant + start_paused + advance() | Virtual time eliminates flaky tests, matches existing retry.rs pattern |

**Key insight:** The circuit breaker state machine itself is simple (~150 lines of transition logic). The complexity is in the concurrency primitives wrapping it. Using DashMap and watch channels avoids hand-rolling concurrent data structures.

## Common Pitfalls

### Pitfall 1: Mutex Held Across Await (Deadlock)

**What goes wrong:** Code acquires the inner Mutex, checks state, finds WaitForProbe, and then tries to `.changed().await` while still holding the lock. The probe task cannot record its result because it also needs the Mutex. Deadlock.

**Why it happens:** Natural to write `let guard = mutex.lock(); if guard.wait_needed { watch.changed().await }` -- the await is inside the lock scope.

**How to avoid:** Extract the check result and watch::Receiver clone inside the lock scope, drop the guard, THEN await. The `check_state()` -> `match result` pattern in the code examples above demonstrates this.

**Warning signs:** `std::sync::Mutex` in an async function with `.await` after `.lock()` in the same scope.

### Pitfall 2: Watch Channel Stale Values

**What goes wrong:** A waiter subscribes to the watch channel and immediately sees a `ProbeResult::Success` from a PREVIOUS probe cycle. It thinks the current probe succeeded and proceeds, but the circuit is actually still in Half-Open waiting for the current probe.

**Why it happens:** `watch` stores the latest value. If a previous probe succeeded (circuit closed, then re-opened, then entered Half-Open again), the watch channel still holds the old Success value.

**How to avoid:** Reset the watch channel by sending `ProbeResult::Pending` when transitioning to Half-Open. Waiters check the value after `changed().await` -- if it's Pending, they continue waiting. Only Success or Failed are terminal.

**Warning signs:** Tests that pass in isolation but fail when probe cycles repeat.

### Pitfall 3: DashMap Entry Lock Held Too Long

**What goes wrong:** Code does `let entry = breakers.get("alpha"); /* long operation */ drop(entry);` -- the DashMap shard lock is held for the entire duration of the "long operation". If another provider hashes to the same shard, it is blocked.

**Why it happens:** DashMap's `get()` and `get_mut()` return `Ref`/`RefMut` guards that hold the shard lock. It is easy to forget that the guard lifetime extends to the end of the scope.

**How to avoid:** Clone needed data out of the DashMap entry, drop the entry reference, then do work. For the circuit breaker: acquire inner Mutex lock, extract check_result, drop Mutex guard, drop DashMap entry, then process the result.

**Warning signs:** DashMap `Ref`/`RefMut` variables with long lifetimes in async functions.

### Pitfall 4: Forgetting to Reset probe_in_flight

**What goes wrong:** The probe permit is acquired (`probe_in_flight = true`) but the probe caller encounters an error outside the circuit breaker (e.g., routing error, serialization error) and never calls `record_probe_success()` or `record_probe_failure()`. The probe_in_flight flag stays true forever. All subsequent requests for this provider wait indefinitely on the watch channel.

**Why it happens:** The probe caller is external code (the handler in Phase 14). If it has a bug or panics before recording the outcome, the circuit breaker is stuck.

**How to avoid:** Provide a `ProbeGuard` RAII type that calls `record_probe_failure()` on Drop if the probe was not explicitly resolved. This ensures the flag is always cleared.

**Example:**
```rust
pub struct ProbeGuard<'a> {
    registry: &'a CircuitBreakerRegistry,
    provider: String,
    resolved: bool,
}

impl<'a> ProbeGuard<'a> {
    pub fn success(mut self) {
        self.resolved = true;
        self.registry.record_probe_success(&self.provider);
    }
    pub fn failure(mut self, reason: &str) {
        self.resolved = true;
        self.registry.record_probe_failure(&self.provider, reason);
    }
}

impl<'a> Drop for ProbeGuard<'a> {
    fn drop(&mut self) {
        if !self.resolved {
            tracing::warn!(provider = %self.provider, "Probe guard dropped without resolution, treating as failure");
            self.registry.record_probe_failure(&self.provider, "probe dropped without resolution");
        }
    }
}
```

### Pitfall 5: Incorrect tokio::time::Instant Usage in Non-Async Tests

**What goes wrong:** Pure `#[test]` (non-async) tests that call `tokio::time::Instant::now()` panic because there is no Tokio runtime context.

**Why it happens:** `tokio::time::Instant` requires an active Tokio runtime. The circuit breaker struct stores `Instant` fields that are initialized in `new()`.

**How to avoid:** All tests that create `ProviderCircuitBreaker` instances must use `#[tokio::test]` (or `#[tokio::test(start_paused = true)]` for time-dependent tests). No pure `#[test]` for any code that touches `tokio::time::Instant`.

**Warning signs:** `thread 'tests::test_name' panicked at 'there is no reactor running'` in test output.

## Code Examples

Verified patterns from official sources and existing codebase:

### Deterministic Time Testing (Existing Pattern)
```rust
// Source: src/proxy/retry.rs lines 476-506
#[tokio::test(start_paused = true)]
async fn test_open_transitions_to_half_open_after_timeout() {
    // Time starts at virtual 0
    let mut inner = CircuitBreakerInner::new();

    // Trip the circuit
    inner.record_failure("timeout");
    inner.record_failure("timeout");
    inner.record_failure("timeout");
    assert_eq!(inner.state, CircuitState::Open);

    // Time has not advanced -- still Open
    assert!(matches!(inner.check_state(), CheckResult::Rejected));

    // Advance past the 30s timeout
    tokio::time::advance(Duration::from_secs(31)).await;

    // Now should transition to HalfOpen
    assert!(matches!(inner.check_state(), CheckResult::ProbePermit));
    assert_eq!(inner.state, CircuitState::HalfOpen);
}
```

### Watch Channel Probe Broadcasting
```rust
// Source: tokio::sync::watch docs
use tokio::sync::watch;

// Create watch channel with initial Pending state
let (tx, _rx) = watch::channel(ProbeResult::Pending);

// Waiter clones a receiver
let mut waiter_rx = tx.subscribe();

// Probe completes and broadcasts result
tx.send(ProbeResult::Success).unwrap();

// Waiter receives the result
waiter_rx.changed().await.unwrap();
assert_eq!(*waiter_rx.borrow(), ProbeResult::Success);
```

### DashMap Entry Access Pattern
```rust
// Source: DashMap docs (https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html)
use dashmap::DashMap;

let map: DashMap<String, ProviderCircuitBreaker> = DashMap::new();

// Short-lived entry access -- get data, drop ref, then work
let check_result = {
    let entry = map.get("provider-alpha").unwrap();
    let mut inner = entry.value().inner.lock().unwrap();
    inner.check_state()
    // inner Mutex guard dropped here
    // DashMap entry ref dropped here
};

// Work with check_result outside of any lock
match check_result {
    CheckResult::WaitForProbe => { /* await probe result */ }
    _ => { /* proceed */ }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Circuit breaker libraries (failsafe, recloser) | Hand-rolled ~250 lines | N/A | Libraries impose call-wrapping APIs that conflict with arbstr's existing retry architecture. Custom is simpler. |
| Sliding window failure rate | Consecutive failure counter | N/A (per user decision) | Simpler, deterministic, appropriate for low-traffic local proxy with 1-10 providers. |
| Background health probes | Lazy Open->HalfOpen on request | N/A (per prior decision) | No background tasks, no wasted API calls, probe uses real traffic. |

**Deprecated/outdated:**
- DashMap 5.x: superseded by 6.x (current stable). No API changes relevant to this use case. Use 6.x.
- DashMap 7.0.0-rc2: release candidate, not stable. Use 6.x for production.

## Open Questions

1. **Watch channel memory under pathological load**
   - What we know: Each waiting task clones a `watch::Receiver`, which is lightweight (internal Arc + generation counter). `watch` stores exactly one value.
   - What's unclear: If hundreds of requests pile up waiting for a probe, each holds a Receiver clone. This should be fine (Receivers are ~32 bytes each), but the exact overhead is not documented.
   - Recommendation: Not a concern at arbstr's scale (local proxy). Monitor in production if provider failures cause request queuing.

2. **ProbeGuard lifetime across async boundaries**
   - What we know: The ProbeGuard must be held by the caller from permit acquisition through probe completion. It calls record_probe_failure() on Drop if not resolved.
   - What's unclear: If the caller's future is cancelled (e.g., client disconnect), Drop fires but may not have access to the actual error reason.
   - Recommendation: The Drop path treats unresolved probes as failures with a generic reason. This is safe -- the circuit reopens, which is the conservative choice.

## Sources

### Primary (HIGH confidence)
- Existing codebase: `src/proxy/retry.rs` (retry_with_fallback, AttemptRecord, Arc<Mutex<Vec>>, start_paused tests), `src/proxy/handlers.rs` (send_to_provider, chat_completions flow, RequestError), `src/proxy/server.rs` (AppState, run_server), `src/router/selector.rs` (select_candidates, SelectedProvider), `src/error.rs` (Error enum, IntoResponse) -- all read and verified
- [DashMap 6.x documentation](https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html) -- per-shard locking, Ref/RefMut guard semantics confirmed
- [DashMap on crates.io](https://crates.io/crates/dashmap) -- v6.1.0 stable, 170M+ downloads
- [tokio::sync::watch documentation](https://docs.rs/tokio/latest/tokio/sync/watch/index.html) -- send/subscribe/changed semantics confirmed
- [tokio::sync::Notify documentation](https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html) -- evaluated and rejected (no data carrying)
- [tokio::time::pause documentation](https://docs.rs/tokio/latest/tokio/time/fn.pause.html) -- start_paused, advance, Instant virtual time behavior confirmed
- Prior project research: `.planning/research/ARCHITECTURE.md`, `.planning/research/STACK.md`, `.planning/research/PITFALLS.md`, `.planning/research/FEATURES.md` -- extensive circuit breaker domain research already completed

### Secondary (MEDIUM confidence)
- [tokio testing guide](https://tokio.rs/tokio/topics/testing) -- start_paused pattern verified
- [Circuit Breaker Pattern - Microsoft Azure Architecture Center](https://learn.microsoft.com/en-us/azure/architecture/patterns/circuit-breaker) -- canonical pattern reference
- [Martin Fowler - Circuit Breaker](https://martinfowler.com/bliki/CircuitBreaker.html) -- original pattern description
- [Shared state - Tokio tutorial](https://tokio.rs/tokio/tutorial/shared-state) -- std::sync::Mutex in async context guidance

### Tertiary (LOW confidence)
- [circuitbreaker-rs crate](https://docs.rs/circuitbreaker-rs) -- evaluated, not using (custom implementation simpler for this architecture)
- [DashMap deadlock case study](https://savannahar68.medium.com/deadlock-issues-in-rusts-dashmap-a-practical-case-study-ad08f10c2849) -- awareness of DashMap deadlock patterns (entry guards held too long)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- DashMap 6 verified as standard concurrent map, tokio::sync::watch verified as correct for multi-receiver signaling, all existing deps already in Cargo.toml
- Architecture: HIGH -- state machine pattern well-established, integration points identified through direct code inspection, queue-and-wait mechanism designed with concrete code examples
- Pitfalls: HIGH -- race conditions in concurrent state machines are well-documented, Mutex-across-await is a known Rust async pitfall, DashMap guard lifetime issue verified through official docs and community case studies

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable domain, no fast-moving dependencies)
