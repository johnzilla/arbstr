//! Circuit breaker state machine for per-provider health tracking.
//!
//! Implements the Closed -> Open -> Half-Open -> Closed lifecycle:
//! - **Closed**: requests flow normally, consecutive failures are counted
//! - **Open**: requests are rejected, waits for timeout to expire
//! - **Half-Open**: a single probe request is allowed to test recovery
//!
//! This module contains the core state machine (`CircuitBreakerInner`) and
//! associated types. The concurrency wrapper (DashMap registry, watch channel,
//! ProbeGuard) is added in Plan 13-02.

use std::time::Duration;

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
        todo!("implement in GREEN phase")
    }

    /// Check whether a request should be allowed through.
    ///
    /// Implements lazy Open -> Half-Open transition when timeout expires.
    pub(crate) fn check_state(&mut self) -> CheckResult {
        todo!("implement in GREEN phase")
    }

    /// Try to acquire the single probe permit in Half-Open state.
    fn try_acquire_probe(&mut self) -> CheckResult {
        todo!("implement in GREEN phase")
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
        todo!("implement in GREEN phase")
    }

    /// Record a success in Closed state. Resets the failure counter.
    pub(crate) fn record_success(&mut self, provider_name: &str) {
        todo!("implement in GREEN phase")
    }

    /// Record that the probe request in Half-Open state succeeded.
    ///
    /// Transitions Half-Open -> Closed.
    pub(crate) fn record_probe_success(&mut self, provider_name: &str) {
        todo!("implement in GREEN phase")
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
        todo!("implement in GREEN phase")
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
}
