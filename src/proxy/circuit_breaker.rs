//! Circuit breaker state machine for per-provider health tracking.
//!
//! Implements the Closed -> Open -> Half-Open -> Closed lifecycle:
//! - **Closed**: requests flow normally, consecutive failures are counted
//! - **Open**: requests are rejected, waits for timeout to expire
//! - **Half-Open**: a single probe request is allowed to test recovery
//!
//! This module contains:
//! - Core state machine (`CircuitBreakerInner`)
//! - Concurrent registry (`CircuitBreakerRegistry`) backed by DashMap
//! - Queue-and-wait probe signaling via `tokio::sync::watch`
//! - RAII `ProbeGuard` to prevent stuck probe_in_flight flags

use dashmap::DashMap;
use std::time::Duration;
use tokio::sync::watch;

/// Number of consecutive failures required to trip the circuit.
const FAILURE_THRESHOLD: u32 = 3;

/// Duration the circuit stays Open before transitioning to Half-Open.
const OPEN_DURATION: Duration = Duration::from_secs(30);

/// The three states of the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation. Requests flow through, failures are counted.
    Closed,
    /// Circuit tripped. All requests are rejected until timeout expires.
    Open,
    /// Recovery probe. One request is allowed through to test provider health.
    HalfOpen,
}

impl CircuitState {
    /// Lowercase string representation for JSON serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            CircuitState::Closed => "closed",
            CircuitState::Open => "open",
            CircuitState::HalfOpen => "half_open",
        }
    }
}

/// Snapshot of a single provider's circuit breaker state.
#[derive(Debug)]
pub struct CircuitSnapshot {
    pub name: String,
    pub state: CircuitState,
    pub failure_count: u32,
}

/// Result of a probe request in Half-Open state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    /// No probe has completed yet.
    Pending,
    /// Probe succeeded, circuit is now Closed.
    Success,
    /// Probe failed, circuit is now Open.
    Failed,
}

/// Result of checking circuit breaker state for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckResult {
    /// Circuit is Closed -- request may proceed.
    Allowed,
    /// Caller has been granted the probe permit (single-permit model).
    ProbePermit,
    /// Another probe is in flight -- caller should wait.
    WaitForProbe,
    /// Circuit is Open and timeout has not expired -- request rejected.
    Rejected,
}

/// Information about the last error that caused a state transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastError {
    /// Category of the error (e.g., "5xx", "timeout").
    pub error_type: String,
    /// Human-readable error message.
    pub message: String,
}

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

/// Core circuit breaker state machine (not thread-safe on its own).
///
/// Plan 13-02 wraps this in `Mutex<CircuitBreakerInner>` inside the
/// `ProviderCircuitBreaker` struct for thread-safe access.
pub(crate) struct CircuitBreakerInner {
    /// Current circuit state.
    pub(crate) state: CircuitState,
    /// Consecutive failure count (resets on success).
    pub(crate) failure_count: u32,
    /// When the circuit transitioned to Open (for timeout calculation).
    pub(crate) opened_at: Option<tokio::time::Instant>,
    /// When the last failure was recorded.
    pub(crate) last_failure_time: Option<tokio::time::Instant>,
    /// When the last success was recorded.
    pub(crate) last_success_time: Option<tokio::time::Instant>,
    /// Details of the most recent error.
    pub(crate) last_error: Option<LastError>,
    /// Total number of times this circuit has tripped open.
    pub(crate) trip_count: u32,
    /// Whether a probe request is currently in flight (Half-Open single-permit).
    pub(crate) probe_in_flight: bool,
}

impl CircuitBreakerInner {
    /// Create a new circuit breaker in the Closed state.
    pub(crate) fn new() -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            opened_at: None,
            last_failure_time: None,
            last_success_time: None,
            last_error: None,
            trip_count: 0,
            probe_in_flight: false,
        }
    }

    /// Check whether a request should be allowed through.
    ///
    /// Implements lazy Open -> Half-Open transition when timeout expires.
    pub(crate) fn check_state(&mut self) -> CheckResult {
        match self.state {
            CircuitState::Closed => CheckResult::Allowed,
            CircuitState::Open => {
                if let Some(opened_at) = self.opened_at {
                    if tokio::time::Instant::now().duration_since(opened_at) >= OPEN_DURATION {
                        // Lazy transition: Open -> HalfOpen
                        self.state = CircuitState::HalfOpen;
                        self.probe_in_flight = false;
                        tracing::info!("circuit entering Half-Open: timeout expired");
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

    /// Try to acquire the single probe permit in Half-Open state.
    fn try_acquire_probe(&mut self) -> CheckResult {
        if !self.probe_in_flight {
            self.probe_in_flight = true;
            CheckResult::ProbePermit
        } else {
            CheckResult::WaitForProbe
        }
    }

    /// Record a failure in Closed state. Only call when circuit is Closed.
    ///
    /// Increments consecutive failure counter. If threshold reached,
    /// transitions to Open state.
    pub(crate) fn record_failure(
        &mut self,
        provider_name: &str,
        error_type: &str,
        message: &str,
    ) {
        self.failure_count += 1;
        self.last_failure_time = Some(tokio::time::Instant::now());
        self.last_error = Some(LastError {
            error_type: error_type.to_string(),
            message: message.to_string(),
        });

        if self.failure_count >= FAILURE_THRESHOLD {
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
    }

    /// Record a success in Closed state. Resets the failure counter.
    pub(crate) fn record_success(&mut self, provider_name: &str) {
        self.failure_count = 0;
        self.last_success_time = Some(tokio::time::Instant::now());

        tracing::debug!(
            provider = %provider_name,
            "circuit breaker: success recorded, failure count reset",
        );
    }

    /// Record that the probe request in Half-Open state succeeded.
    ///
    /// Transitions Half-Open -> Closed.
    pub(crate) fn record_probe_success(&mut self, provider_name: &str) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.probe_in_flight = false;
        self.last_success_time = Some(tokio::time::Instant::now());

        tracing::info!(
            provider = %provider_name,
            trip_count = self.trip_count,
            "circuit CLOSED: probe succeeded",
        );
    }

    /// Record that the probe request in Half-Open state failed.
    ///
    /// Transitions Half-Open -> Open with a fresh timeout.
    pub(crate) fn record_probe_failure(
        &mut self,
        provider_name: &str,
        error_type: &str,
        message: &str,
    ) {
        self.state = CircuitState::Open;
        self.opened_at = Some(tokio::time::Instant::now());
        self.probe_in_flight = false;
        self.last_error = Some(LastError {
            error_type: error_type.to_string(),
            message: message.to_string(),
        });

        tracing::warn!(
            provider = %provider_name,
            trip_count = self.trip_count,
            "circuit REOPENED: probe failed",
        );
    }
}

// ── Permit type ──────────────────────────────────────────────────────

/// Type of permit returned by [`CircuitBreakerRegistry::acquire_permit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermitType {
    /// Normal request through a closed circuit.
    Normal,
    /// Probe request through a half-open circuit. Caller MUST use [`ProbeGuard`].
    Probe,
}

// ── Per-provider circuit breaker ─────────────────────────────────────

/// Thread-safe wrapper around [`CircuitBreakerInner`] with probe result
/// signaling via `tokio::sync::watch`.
pub(crate) struct ProviderCircuitBreaker {
    inner: std::sync::Mutex<CircuitBreakerInner>,
    /// Watch channel for broadcasting probe results to waiting tasks.
    /// Initialized with `ProbeResult::Pending`.
    probe_watch: watch::Sender<ProbeResult>,
}

impl ProviderCircuitBreaker {
    /// Create a new provider circuit breaker in the Closed state.
    fn new() -> Self {
        let (tx, _rx) = watch::channel(ProbeResult::Pending);
        Self {
            inner: std::sync::Mutex::new(CircuitBreakerInner::new()),
            probe_watch: tx,
        }
    }
}

// ── Registry ─────────────────────────────────────────────────────────

/// Concurrent circuit breaker registry with one breaker per provider.
///
/// Backed by [`DashMap`] for per-shard locking (no cross-provider contention).
/// Provides `acquire_permit` with queue-and-wait semantics during Half-Open
/// probing.
pub struct CircuitBreakerRegistry {
    breakers: DashMap<String, ProviderCircuitBreaker>,
}

impl CircuitBreakerRegistry {
    /// Create a registry with one [`ProviderCircuitBreaker`] per provider name.
    ///
    /// All breakers start in Closed state.
    pub fn new(provider_names: &[String]) -> Self {
        let breakers = DashMap::with_capacity(provider_names.len());
        for name in provider_names {
            breakers.insert(name.clone(), ProviderCircuitBreaker::new());
        }
        Self { breakers }
    }

    /// Check whether a request to `provider_name` should proceed.
    ///
    /// Returns `Ok(PermitType::Normal)` for closed circuits,
    /// `Ok(PermitType::Probe)` for the single half-open probe permit,
    /// or `Err(CircuitOpenError)` for open circuits.
    ///
    /// When a probe is already in flight, this method **waits** for the
    /// probe result via `tokio::sync::watch` (queue-and-wait semantics).
    ///
    /// Unknown providers are allowed through (circuit breaker is opt-in
    /// for configured providers).
    pub async fn acquire_permit(
        &self,
        provider_name: &str,
    ) -> Result<PermitType, CircuitOpenError> {
        let Some(entry) = self.breakers.get(provider_name) else {
            // Unknown provider -- allow (circuit breaker is opt-in)
            return Ok(PermitType::Normal);
        };

        let cb = entry.value();

        // Lock inner, extract check result and any data needed for error/wait.
        // CRITICAL: Mutex and DashMap entry are dropped before any .await.
        let (check_result, error_info, mut rx) = {
            let mut inner = cb.inner.lock().unwrap();
            let result = inner.check_state();
            let err_info = (
                inner
                    .last_error
                    .as_ref()
                    .map(|e| format!("{}: {}", e.error_type, e.message))
                    .unwrap_or_else(|| "unknown".to_string()),
                inner.trip_count,
            );
            let receiver = cb.probe_watch.subscribe();
            (result, err_info, receiver)
        };
        // DashMap entry ref dropped here
        drop(entry);

        match check_result {
            CheckResult::Allowed => Ok(PermitType::Normal),
            CheckResult::ProbePermit => Ok(PermitType::Probe),
            CheckResult::Rejected => Err(CircuitOpenError {
                provider: provider_name.to_string(),
                reason: error_info.0,
                trip_count: error_info.1,
            }),
            CheckResult::WaitForProbe => {
                // Wait for probe result outside of all locks
                loop {
                    // Wait for a new value to be sent
                    if rx.changed().await.is_err() {
                        // Sender dropped -- treat as failure
                        return Err(CircuitOpenError {
                            provider: provider_name.to_string(),
                            reason: "probe watch channel closed".to_string(),
                            trip_count: error_info.1,
                        });
                    }
                    let result = *rx.borrow();
                    match result {
                        ProbeResult::Success => return Ok(PermitType::Normal),
                        ProbeResult::Failed => {
                            return Err(CircuitOpenError {
                                provider: provider_name.to_string(),
                                reason: error_info.0.clone(),
                                trip_count: error_info.1,
                            });
                        }
                        ProbeResult::Pending => {
                            // Reset for next cycle -- continue waiting
                            continue;
                        }
                    }
                }
            }
        }
    }

    /// Record a successful request for `provider_name` (Closed state).
    ///
    /// Resets the consecutive failure counter.
    pub fn record_success(&self, provider_name: &str) {
        if let Some(entry) = self.breakers.get(provider_name) {
            let mut inner = entry.value().inner.lock().unwrap();
            inner.record_success(provider_name);
        }
    }

    /// Record a failed request for `provider_name` (Closed state).
    ///
    /// Increments failure counter; may trip the circuit to Open.
    pub fn record_failure(&self, provider_name: &str, error_type: &str, message: &str) {
        if let Some(entry) = self.breakers.get(provider_name) {
            let mut inner = entry.value().inner.lock().unwrap();
            inner.record_failure(provider_name, error_type, message);
        }
    }

    /// Record that the half-open probe succeeded for `provider_name`.
    ///
    /// Transitions the circuit to Closed and broadcasts `ProbeResult::Success`
    /// to all waiting tasks. The watch channel is NOT reset to Pending here;
    /// stale values are prevented by `subscribe()` semantics -- new subscribers
    /// mark the current value as seen and only wake on subsequent sends.
    pub fn record_probe_success(&self, provider_name: &str) {
        if let Some(entry) = self.breakers.get(provider_name) {
            let cb = entry.value();
            let mut inner = cb.inner.lock().unwrap();
            inner.record_probe_success(provider_name);
            let _ = cb.probe_watch.send(ProbeResult::Success);
        }
    }

    /// Record that the half-open probe failed for `provider_name`.
    ///
    /// Transitions the circuit to Open and broadcasts `ProbeResult::Failed`
    /// to all waiting tasks. The watch channel is NOT reset to Pending here;
    /// stale values are prevented by `subscribe()` semantics.
    pub fn record_probe_failure(&self, provider_name: &str, error_type: &str, message: &str) {
        if let Some(entry) = self.breakers.get(provider_name) {
            let cb = entry.value();
            let mut inner = cb.inner.lock().unwrap();
            inner.record_probe_failure(provider_name, error_type, message);
            let _ = cb.probe_watch.send(ProbeResult::Failed);
        }
    }

    /// Return a snapshot of all provider circuit states.
    ///
    /// Uses DashMap::iter() which acquires per-shard locks (not a global lock).
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

    /// Read-only accessor for cumulative trip count (for Phase 15).
    pub fn trip_count(&self, provider_name: &str) -> Option<u32> {
        self.breakers
            .get(provider_name)
            .map(|entry| entry.value().inner.lock().unwrap().trip_count)
    }
}

// ── ProbeGuard RAII ──────────────────────────────────────────────────

/// RAII guard that ensures a half-open probe is always resolved.
///
/// If dropped without calling [`success`](ProbeGuard::success) or
/// [`failure`](ProbeGuard::failure), the probe is treated as a failure
/// to prevent stuck `probe_in_flight` flags.
pub struct ProbeGuard<'a> {
    registry: &'a CircuitBreakerRegistry,
    provider: String,
    resolved: bool,
}

impl<'a> ProbeGuard<'a> {
    /// Create a new probe guard for the given provider.
    pub fn new(registry: &'a CircuitBreakerRegistry, provider: String) -> Self {
        Self {
            registry,
            provider,
            resolved: false,
        }
    }

    /// Mark the probe as successful. Closes the circuit.
    pub fn success(mut self) {
        self.resolved = true;
        self.registry.record_probe_success(&self.provider);
    }

    /// Mark the probe as failed. Reopens the circuit with a fresh timer.
    pub fn failure(mut self, error_type: &str, message: &str) {
        self.resolved = true;
        self.registry
            .record_probe_failure(&self.provider, error_type, message);
    }
}

impl<'a> Drop for ProbeGuard<'a> {
    fn drop(&mut self) {
        if !self.resolved {
            tracing::warn!(
                provider = %self.provider,
                "ProbeGuard dropped without resolution, treating as failure"
            );
            self.registry.record_probe_failure(
                &self.provider,
                "dropped",
                "probe dropped without resolution",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // Helper: trip the circuit by recording FAILURE_THRESHOLD consecutive failures
    fn trip_circuit(cb: &mut CircuitBreakerInner) {
        for _ in 0..FAILURE_THRESHOLD {
            cb.record_failure("test-provider", "5xx", "Internal Server Error");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_initial_state() {
        let cb = CircuitBreakerInner::new();
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.failure_count, 0);
        assert_eq!(cb.trip_count, 0);
        assert!(cb.opened_at.is_none());
        assert!(cb.last_failure_time.is_none());
        assert!(cb.last_success_time.is_none());
        assert!(cb.last_error.is_none());
        assert!(!cb.probe_in_flight);
    }

    #[tokio::test(start_paused = true)]
    async fn test_single_failure_stays_closed() {
        let mut cb = CircuitBreakerInner::new();
        cb.record_failure("test-provider", "5xx", "Internal Server Error");
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.failure_count, 1);
        assert_eq!(cb.trip_count, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn test_two_failures_stays_closed() {
        let mut cb = CircuitBreakerInner::new();
        cb.record_failure("test-provider", "5xx", "Bad Gateway");
        cb.record_failure("test-provider", "timeout", "Request timed out");
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.failure_count, 2);
        assert_eq!(cb.trip_count, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn test_three_failures_opens_circuit() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);
        assert_eq!(cb.state, CircuitState::Open);
        assert_eq!(cb.failure_count, FAILURE_THRESHOLD);
        assert_eq!(cb.trip_count, 1);
        assert!(cb.opened_at.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn test_success_resets_failure_count() {
        let mut cb = CircuitBreakerInner::new();

        // 2 failures
        cb.record_failure("test-provider", "5xx", "Error 1");
        cb.record_failure("test-provider", "5xx", "Error 2");
        assert_eq!(cb.failure_count, 2);

        // 1 success resets counter
        cb.record_success("test-provider");
        assert_eq!(cb.failure_count, 0);
        assert_eq!(cb.state, CircuitState::Closed);

        // 2 more failures -- still Closed because they are not consecutive with the first 2
        cb.record_failure("test-provider", "5xx", "Error 3");
        cb.record_failure("test-provider", "5xx", "Error 4");
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.failure_count, 2);
        assert_eq!(cb.trip_count, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn test_open_rejects_requests() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);
        assert_eq!(cb.state, CircuitState::Open);

        // Check state should return Rejected (timeout hasn't expired)
        let result = cb.check_state();
        assert_eq!(result, CheckResult::Rejected);
        assert_eq!(cb.state, CircuitState::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn test_open_transitions_to_half_open_after_timeout() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);
        assert_eq!(cb.state, CircuitState::Open);

        // Advance past the 30s timeout
        tokio::time::advance(Duration::from_secs(31)).await;

        // Should transition to HalfOpen and return ProbePermit
        let result = cb.check_state();
        assert_eq!(result, CheckResult::ProbePermit);
        assert_eq!(cb.state, CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_open_stays_open_before_timeout() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);
        assert_eq!(cb.state, CircuitState::Open);

        // Advance only 29s -- not past timeout
        tokio::time::advance(Duration::from_secs(29)).await;

        let result = cb.check_state();
        assert_eq!(result, CheckResult::Rejected);
        assert_eq!(cb.state, CircuitState::Open);
    }

    #[tokio::test(start_paused = true)]
    async fn test_half_open_single_probe_permit() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);

        // Advance past timeout to transition to HalfOpen
        tokio::time::advance(Duration::from_secs(31)).await;

        // First check gets ProbePermit
        let first = cb.check_state();
        assert_eq!(first, CheckResult::ProbePermit);
        assert_eq!(cb.state, CircuitState::HalfOpen);

        // Second check gets WaitForProbe (probe already in flight)
        let second = cb.check_state();
        assert_eq!(second, CheckResult::WaitForProbe);
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_success_closes_circuit() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);

        // Transition to HalfOpen
        tokio::time::advance(Duration::from_secs(31)).await;
        let result = cb.check_state();
        assert_eq!(result, CheckResult::ProbePermit);

        // Probe succeeds
        cb.record_probe_success("test-provider");
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.failure_count, 0);
        assert!(!cb.probe_in_flight);
        assert!(cb.last_success_time.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_failure_reopens_circuit() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);

        // Transition to HalfOpen
        tokio::time::advance(Duration::from_secs(31)).await;
        let _result = cb.check_state();

        let opened_at_before = cb.opened_at;

        // Probe fails
        cb.record_probe_failure("test-provider", "5xx", "Still broken");
        assert_eq!(cb.state, CircuitState::Open);
        assert!(!cb.probe_in_flight);

        // opened_at should be fresh (different from original)
        assert!(cb.opened_at.is_some());
        assert_ne!(cb.opened_at, opened_at_before);
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_failure_resets_timer() {
        let mut cb = CircuitBreakerInner::new();
        trip_circuit(&mut cb);

        // First timeout: advance 31s to HalfOpen
        tokio::time::advance(Duration::from_secs(31)).await;
        let _result = cb.check_state();
        assert_eq!(cb.state, CircuitState::HalfOpen);

        // Probe fails -- circuit reopens with fresh timer
        cb.record_probe_failure("test-provider", "5xx", "Still down");
        assert_eq!(cb.state, CircuitState::Open);

        // Advance another 31s from the probe failure -- should transition again
        tokio::time::advance(Duration::from_secs(31)).await;
        let result = cb.check_state();
        assert_eq!(result, CheckResult::ProbePermit);
        assert_eq!(cb.state, CircuitState::HalfOpen);
    }

    #[tokio::test(start_paused = true)]
    async fn test_trip_count_increments() {
        let mut cb = CircuitBreakerInner::new();

        // First trip
        trip_circuit(&mut cb);
        assert_eq!(cb.trip_count, 1);

        // Recover: transition to HalfOpen, probe success
        tokio::time::advance(Duration::from_secs(31)).await;
        let _result = cb.check_state();
        cb.record_probe_success("test-provider");
        assert_eq!(cb.state, CircuitState::Closed);
        assert_eq!(cb.trip_count, 1);

        // Second trip
        trip_circuit(&mut cb);
        assert_eq!(cb.state, CircuitState::Open);
        assert_eq!(cb.trip_count, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn test_last_error_tracked() {
        let mut cb = CircuitBreakerInner::new();

        cb.record_failure("test-provider", "5xx", "First error");
        assert_eq!(
            cb.last_error,
            Some(LastError {
                error_type: "5xx".to_string(),
                message: "First error".to_string(),
            })
        );

        cb.record_failure("test-provider", "timeout", "Second error");
        assert_eq!(
            cb.last_error,
            Some(LastError {
                error_type: "timeout".to_string(),
                message: "Second error".to_string(),
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_timestamps_tracked() {
        let mut cb = CircuitBreakerInner::new();

        // Record a failure -- should set last_failure_time
        cb.record_failure("test-provider", "5xx", "Error");
        assert!(cb.last_failure_time.is_some());

        // Record a success -- should set last_success_time
        cb.record_success("test-provider");
        assert!(cb.last_success_time.is_some());

        // Trip circuit -- should set opened_at
        trip_circuit(&mut cb);
        assert!(cb.opened_at.is_some());
        let opened_at = cb.opened_at.unwrap();

        // Verify opened_at is recent (within this test's virtual time)
        let now = tokio::time::Instant::now();
        assert!(now.duration_since(opened_at) < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn test_check_result_values() {
        // Verify all four CheckResult variants are distinct
        let allowed = CheckResult::Allowed;
        let probe_permit = CheckResult::ProbePermit;
        let wait_for_probe = CheckResult::WaitForProbe;
        let rejected = CheckResult::Rejected;

        assert_ne!(allowed, probe_permit);
        assert_ne!(allowed, wait_for_probe);
        assert_ne!(allowed, rejected);
        assert_ne!(probe_permit, wait_for_probe);
        assert_ne!(probe_permit, rejected);
        assert_ne!(wait_for_probe, rejected);

        // Verify a Closed circuit returns Allowed
        let mut cb = CircuitBreakerInner::new();
        assert_eq!(cb.check_state(), CheckResult::Allowed);
    }

    // ── Registry tests ───────────────────────────────────────────────

    /// Helper: trip a provider's circuit through the registry
    fn trip_registry(registry: &CircuitBreakerRegistry, provider: &str) {
        for _ in 0..FAILURE_THRESHOLD {
            registry.record_failure(provider, "5xx", "Internal Server Error");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_new_creates_breakers() {
        let names = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "gamma".to_string(),
        ];
        let registry = CircuitBreakerRegistry::new(&names);

        assert_eq!(registry.state("alpha"), Some(CircuitState::Closed));
        assert_eq!(registry.state("beta"), Some(CircuitState::Closed));
        assert_eq!(registry.state("gamma"), Some(CircuitState::Closed));
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_unknown_provider_allowed() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        let result = registry.acquire_permit("unknown-provider").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PermitType::Normal);
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_acquire_permit_closed() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        let result = registry.acquire_permit("alpha").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PermitType::Normal);
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_acquire_permit_open_rejected() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        trip_registry(&registry, "alpha");

        let result = registry.acquire_permit("alpha").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.provider, "alpha");
        assert_eq!(err.trip_count, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_record_success_resets() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);

        // 2 failures
        registry.record_failure("alpha", "5xx", "Error 1");
        registry.record_failure("alpha", "5xx", "Error 2");
        assert_eq!(registry.failure_count("alpha"), Some(2));

        // 1 success resets counter
        registry.record_success("alpha");
        assert_eq!(registry.failure_count("alpha"), Some(0));

        // 2 more failures -- still Closed (not consecutive with first 2)
        registry.record_failure("alpha", "5xx", "Error 3");
        registry.record_failure("alpha", "5xx", "Error 4");
        assert_eq!(registry.state("alpha"), Some(CircuitState::Closed));
        assert_eq!(registry.failure_count("alpha"), Some(2));
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_probe_permit_after_timeout() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        trip_registry(&registry, "alpha");

        // Advance past 30s timeout
        tokio::time::advance(Duration::from_secs(31)).await;

        let result = registry.acquire_permit("alpha").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PermitType::Probe);
        assert_eq!(registry.state("alpha"), Some(CircuitState::HalfOpen));
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_queue_and_wait_success() {
        let registry = std::sync::Arc::new(CircuitBreakerRegistry::new(&["alpha".to_string()]));
        trip_registry(&registry, "alpha");

        // Advance past timeout
        tokio::time::advance(Duration::from_secs(31)).await;

        // First acquire gets Probe permit
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Spawn a waiter that will block on WaitForProbe
        let reg_clone = registry.clone();
        let waiter = tokio::spawn(async move { reg_clone.acquire_permit("alpha").await });

        // Let the waiter task start and reach the watch channel
        tokio::task::yield_now().await;

        // Probe succeeds -- waiter should get Ok(Normal)
        registry.record_probe_success("alpha");

        let waiter_result = waiter.await.unwrap();
        assert!(waiter_result.is_ok());
        assert_eq!(waiter_result.unwrap(), PermitType::Normal);
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_queue_and_wait_failure() {
        let registry = std::sync::Arc::new(CircuitBreakerRegistry::new(&["alpha".to_string()]));
        trip_registry(&registry, "alpha");

        // Advance past timeout
        tokio::time::advance(Duration::from_secs(31)).await;

        // First acquire gets Probe permit
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Spawn a waiter
        let reg_clone = registry.clone();
        let waiter = tokio::spawn(async move { reg_clone.acquire_permit("alpha").await });

        tokio::task::yield_now().await;

        // Probe fails -- waiter should get Err
        registry.record_probe_failure("alpha", "5xx", "Still broken");

        let waiter_result = waiter.await.unwrap();
        assert!(waiter_result.is_err());
        assert_eq!(waiter_result.unwrap_err().provider, "alpha");
    }

    #[tokio::test(start_paused = true)]
    async fn test_registry_multiple_waiters() {
        let registry = std::sync::Arc::new(CircuitBreakerRegistry::new(&["alpha".to_string()]));
        trip_registry(&registry, "alpha");

        tokio::time::advance(Duration::from_secs(31)).await;

        // First acquire gets Probe
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Spawn 5 waiters
        let mut waiters = Vec::new();
        for _ in 0..5 {
            let reg_clone = registry.clone();
            waiters.push(tokio::spawn(
                async move { reg_clone.acquire_permit("alpha").await },
            ));
        }

        // Let all waiters reach the watch channel
        tokio::task::yield_now().await;

        // Probe succeeds
        registry.record_probe_success("alpha");

        // All 5 should receive Ok(Normal)
        for waiter in waiters {
            let result = waiter.await.unwrap();
            assert!(result.is_ok(), "All waiters should succeed");
            assert_eq!(result.unwrap(), PermitType::Normal);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_guard_success() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        trip_registry(&registry, "alpha");

        tokio::time::advance(Duration::from_secs(31)).await;
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Create ProbeGuard and call success
        let guard = ProbeGuard::new(&registry, "alpha".to_string());
        guard.success();

        // Circuit should be Closed
        assert_eq!(registry.state("alpha"), Some(CircuitState::Closed));
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_guard_failure() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        trip_registry(&registry, "alpha");

        tokio::time::advance(Duration::from_secs(31)).await;
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Create ProbeGuard and call failure
        let guard = ProbeGuard::new(&registry, "alpha".to_string());
        guard.failure("5xx", "Still broken");

        // Circuit should be Open (reopened)
        assert_eq!(registry.state("alpha"), Some(CircuitState::Open));
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_guard_drop_without_resolution() {
        let registry = CircuitBreakerRegistry::new(&["alpha".to_string()]);
        trip_registry(&registry, "alpha");

        tokio::time::advance(Duration::from_secs(31)).await;
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Create ProbeGuard and DROP without resolving (RAII safety)
        {
            let _guard = ProbeGuard::new(&registry, "alpha".to_string());
            // guard drops here
        }

        // Circuit should be Open (failure on drop)
        assert_eq!(registry.state("alpha"), Some(CircuitState::Open));
    }

    #[tokio::test(start_paused = true)]
    async fn test_probe_result_reset_prevents_stale() {
        let registry = std::sync::Arc::new(CircuitBreakerRegistry::new(&["alpha".to_string()]));

        // First trip -> recover
        trip_registry(&registry, "alpha");
        tokio::time::advance(Duration::from_secs(31)).await;
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);
        registry.record_probe_success("alpha");
        assert_eq!(registry.state("alpha"), Some(CircuitState::Closed));

        // Second trip
        trip_registry(&registry, "alpha");
        assert_eq!(registry.state("alpha"), Some(CircuitState::Open));

        // Advance past timeout again
        tokio::time::advance(Duration::from_secs(31)).await;

        // First acquire gets Probe
        let result = registry.acquire_permit("alpha").await;
        assert_eq!(result.unwrap(), PermitType::Probe);

        // Spawn a waiter -- should NOT see stale Success from first cycle
        let reg_clone = registry.clone();
        let waiter = tokio::spawn(async move { reg_clone.acquire_permit("alpha").await });

        tokio::task::yield_now().await;

        // This time probe fails
        registry.record_probe_failure("alpha", "5xx", "Down again");

        // Waiter should get Err (not stale Success from first cycle)
        let waiter_result = waiter.await.unwrap();
        assert!(
            waiter_result.is_err(),
            "Waiter should see failure, not stale success from previous cycle"
        );
    }
}
