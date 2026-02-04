//! Retry and fallback logic for non-streaming requests.
//!
//! This module encapsulates the retry-with-fallback algorithm:
//! - Up to `MAX_RETRIES` retries on the primary provider with exponential backoff
//! - Single fallback attempt on the next candidate if primary exhausts retries
//! - Attempt tracking via shared `Arc<Mutex<Vec<AttemptRecord>>>` that survives timeout cancellation
//! - Header formatting for `x-arbstr-retries`

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Fixed exponential backoff: 1s, 2s, 4s.
///
/// Matches the locked decision from CONTEXT.md. With `MAX_RETRIES=2`, only the
/// first two slots (1s, 2s) are used at runtime. The 4s entry documents the full
/// sequence and would be used if `MAX_RETRIES` were increased.
const BACKOFF_DURATIONS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Maximum number of retries on the primary provider (3 total attempts).
const MAX_RETRIES: u32 = 2;

/// Record of a single failed attempt for building the `x-arbstr-retries` header.
#[derive(Debug, Clone)]
pub struct AttemptRecord {
    pub provider_name: String,
    pub status_code: u16,
}

/// Lightweight candidate info for the retry module.
///
/// Decoupled from `SelectedProvider` so the retry logic can be tested
/// without depending on router types.
#[derive(Debug, Clone)]
pub struct CandidateInfo {
    pub name: String,
}

/// Trait for extracting an HTTP status code from an error.
///
/// Allows the retry module to inspect error status codes without
/// depending on `RequestError` directly.
pub trait HasStatusCode {
    fn status_code(&self) -> u16;
}

/// Outcome of the full retry+fallback sequence.
///
/// Generic over `T` (success type) and `E` (error type) so it can be
/// tested without depending on handler types. The handler integration
/// will use `RetryOutcome<RequestOutcome, RequestError>`.
///
/// Note: This does NOT contain an `attempts` field. Attempts are tracked
/// via the shared `Arc<Mutex<Vec<AttemptRecord>>>` parameter passed into
/// `retry_with_fallback`. This design ensures attempt history survives
/// timeout cancellation.
pub struct RetryOutcome<T, E> {
    pub result: std::result::Result<T, E>,
}

/// Whether an HTTP status code should trigger a retry.
///
/// Returns `true` for 500, 502, 503, 504 (server errors that are typically transient).
/// Returns `false` for all other codes including 4xx (permanent client errors).
pub fn is_retryable(status_code: u16) -> bool {
    matches!(status_code, 500 | 502 | 503 | 504)
}

/// Format attempt records into the `x-arbstr-retries` header value.
///
/// Format: `"2/provider-alpha, 1/provider-beta"` -- count of failed attempts
/// per provider, preserving first-appearance order.
///
/// Returns `None` if the attempts slice is empty (no retries occurred).
pub fn format_retries_header(attempts: &[AttemptRecord]) -> Option<String> {
    if attempts.is_empty() {
        return None;
    }
    // Preserve order of first appearance
    let mut counts: Vec<(&str, u32)> = Vec::new();
    for attempt in attempts {
        if let Some(entry) = counts
            .iter_mut()
            .find(|(name, _)| *name == attempt.provider_name)
        {
            entry.1 += 1;
        } else {
            counts.push((&attempt.provider_name, 1));
        }
    }
    Some(
        counts
            .iter()
            .map(|(name, count)| format!("{}/{}", count, name))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// Execute a request with retry on the primary provider and fallback to the next candidate.
///
/// Algorithm:
/// 1. Take first candidate as primary, second (if exists) as fallback
/// 2. Attempt primary up to `MAX_RETRIES + 1` times (3 total)
/// 3. On success: return immediately
/// 4. On error: record attempt in shared vec, check retryability
/// 5. On non-retryable error: return immediately (no retry, no fallback)
/// 6. After primary exhausted with retryable errors: try fallback once
/// 7. If no fallback exists: return last primary error
///
/// The `attempts` parameter is an `Arc<Mutex<Vec<AttemptRecord>>>` that the caller
/// creates and owns. Failed attempts are pushed into this shared vec. This design
/// allows the caller to read accumulated attempts even if this future is cancelled
/// by a timeout.
pub async fn retry_with_fallback<T, E, F, Fut>(
    candidates: &[CandidateInfo],
    attempts: Arc<Mutex<Vec<AttemptRecord>>>,
    send_request: F,
) -> RetryOutcome<T, E>
where
    E: HasStatusCode,
    F: Fn(&CandidateInfo) -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    assert!(
        !candidates.is_empty(),
        "retry_with_fallback requires at least one candidate"
    );

    let primary = &candidates[0];
    let mut last_error: Option<E> = None;

    // Primary provider: up to MAX_RETRIES + 1 total attempts
    for attempt in 0..=MAX_RETRIES {
        // Backoff before retry (not before first attempt)
        if attempt > 0 {
            tokio::time::sleep(BACKOFF_DURATIONS[(attempt - 1) as usize]).await;
        }

        match send_request(primary).await {
            Ok(value) => {
                return RetryOutcome { result: Ok(value) };
            }
            Err(err) => {
                let retryable = is_retryable(err.status_code());

                // Record failed attempt in shared vec
                attempts.lock().unwrap().push(AttemptRecord {
                    provider_name: primary.name.clone(),
                    status_code: err.status_code(),
                });

                if !retryable {
                    // Non-retryable error: fail immediately, no fallback
                    return RetryOutcome { result: Err(err) };
                }

                last_error = Some(err);

                // If not the last attempt, loop continues with backoff
            }
        }
    }

    // Primary exhausted with retryable errors -- try fallback if available
    if candidates.len() > 1 {
        let fallback = &candidates[1];

        match send_request(fallback).await {
            Ok(value) => {
                return RetryOutcome { result: Ok(value) };
            }
            Err(err) => {
                // Record fallback failure
                attempts.lock().unwrap().push(AttemptRecord {
                    provider_name: fallback.name.clone(),
                    status_code: err.status_code(),
                });

                return RetryOutcome { result: Err(err) };
            }
        }
    }

    // No fallback available -- return last primary error
    RetryOutcome {
        result: Err(last_error.expect("at least one attempt was made")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Mock error type for testing.
    #[derive(Debug)]
    struct MockError {
        code: u16,
    }

    impl HasStatusCode for MockError {
        fn status_code(&self) -> u16 {
            self.code
        }
    }

    #[test]
    fn test_is_retryable() {
        // Retryable: 5xx server errors
        assert!(is_retryable(500));
        assert!(is_retryable(502));
        assert!(is_retryable(503));
        assert!(is_retryable(504));

        // Not retryable: 4xx client errors
        assert!(!is_retryable(400));
        assert!(!is_retryable(401));
        assert!(!is_retryable(403));
        assert!(!is_retryable(404));
        assert!(!is_retryable(429));

        // Not retryable: other codes
        assert!(!is_retryable(200));
        assert!(!is_retryable(301));
        assert!(!is_retryable(501)); // 501 Not Implemented is not transient
    }

    #[test]
    fn test_format_retries_header_empty() {
        assert_eq!(format_retries_header(&[]), None);
    }

    #[test]
    fn test_format_retries_header_single_provider() {
        let attempts = vec![
            AttemptRecord {
                provider_name: "alpha".to_string(),
                status_code: 503,
            },
            AttemptRecord {
                provider_name: "alpha".to_string(),
                status_code: 502,
            },
        ];
        assert_eq!(
            format_retries_header(&attempts),
            Some("2/alpha".to_string())
        );
    }

    #[test]
    fn test_format_retries_header_multiple_providers() {
        let attempts = vec![
            AttemptRecord {
                provider_name: "alpha".to_string(),
                status_code: 503,
            },
            AttemptRecord {
                provider_name: "alpha".to_string(),
                status_code: 503,
            },
            AttemptRecord {
                provider_name: "beta".to_string(),
                status_code: 500,
            },
        ];
        assert_eq!(
            format_retries_header(&attempts),
            Some("2/alpha, 1/beta".to_string())
        );
    }

    #[tokio::test]
    async fn test_success_on_first_attempt() {
        let candidates = vec![CandidateInfo {
            name: "alpha".to_string(),
        }];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |_info| {
                let cc = call_count_inner.clone();
                async move {
                    cc.fetch_add(1, Ordering::Relaxed);
                    Ok("success".to_string())
                }
            },
        )
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.result.unwrap(), "success");
        assert_eq!(call_count.load(Ordering::Relaxed), 1);
        assert!(attempts.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn test_retry_then_success() {
        let candidates = vec![CandidateInfo {
            name: "alpha".to_string(),
        }];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |_info| {
                let cc = call_count_inner.clone();
                async move {
                    let n = cc.fetch_add(1, Ordering::Relaxed);
                    if n == 0 {
                        Err(MockError { code: 503 })
                    } else {
                        Ok("recovered".to_string())
                    }
                }
            },
        )
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.result.unwrap(), "recovered");
        assert_eq!(call_count.load(Ordering::Relaxed), 2);
        let recorded = attempts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].provider_name, "alpha");
        assert_eq!(recorded[0].status_code, 503);
    }

    #[tokio::test(start_paused = true)]
    async fn test_max_retries_exhausted_no_fallback() {
        let candidates = vec![CandidateInfo {
            name: "alpha".to_string(),
        }];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |_info| {
                let cc = call_count_inner.clone();
                async move {
                    cc.fetch_add(1, Ordering::Relaxed);
                    Err(MockError { code: 503 })
                }
            },
        )
        .await;

        assert!(outcome.result.is_err());
        assert_eq!(outcome.result.unwrap_err().code, 503);
        // 3 total attempts: 1 initial + 2 retries
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
        let recorded = attempts.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        for record in recorded.iter() {
            assert_eq!(record.provider_name, "alpha");
            assert_eq!(record.status_code, 503);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_max_retries_then_fallback_success() {
        let candidates = vec![
            CandidateInfo {
                name: "alpha".to_string(),
            },
            CandidateInfo {
                name: "beta".to_string(),
            },
        ];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |info| {
                let cc = call_count_inner.clone();
                let name = info.name.clone();
                async move {
                    cc.fetch_add(1, Ordering::Relaxed);
                    if name == "alpha" {
                        Err(MockError { code: 503 })
                    } else {
                        Ok("fallback-success".to_string())
                    }
                }
            },
        )
        .await;

        assert!(outcome.result.is_ok());
        assert_eq!(outcome.result.unwrap(), "fallback-success");
        // 3 primary attempts + 1 fallback = 4 total calls
        assert_eq!(call_count.load(Ordering::Relaxed), 4);
        // Only failed attempts recorded (3 primary failures)
        let recorded = attempts.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        for record in recorded.iter() {
            assert_eq!(record.provider_name, "alpha");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_max_retries_then_fallback_failure() {
        let candidates = vec![
            CandidateInfo {
                name: "alpha".to_string(),
            },
            CandidateInfo {
                name: "beta".to_string(),
            },
        ];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |_info| {
                let cc = call_count_inner.clone();
                async move {
                    cc.fetch_add(1, Ordering::Relaxed);
                    Err(MockError { code: 500 })
                }
            },
        )
        .await;

        assert!(outcome.result.is_err());
        // 3 primary + 1 fallback = 4 total calls
        assert_eq!(call_count.load(Ordering::Relaxed), 4);
        // 3 primary + 1 fallback = 4 recorded attempts
        let recorded = attempts.lock().unwrap();
        assert_eq!(recorded.len(), 4);
        assert_eq!(recorded[0].provider_name, "alpha");
        assert_eq!(recorded[1].provider_name, "alpha");
        assert_eq!(recorded[2].provider_name, "alpha");
        assert_eq!(recorded[3].provider_name, "beta");
    }

    #[tokio::test]
    async fn test_non_retryable_fails_immediately() {
        let candidates = vec![
            CandidateInfo {
                name: "alpha".to_string(),
            },
            CandidateInfo {
                name: "beta".to_string(),
            },
        ];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |_info| {
                let cc = call_count_inner.clone();
                async move {
                    cc.fetch_add(1, Ordering::Relaxed);
                    Err(MockError { code: 400 })
                }
            },
        )
        .await;

        assert!(outcome.result.is_err());
        assert_eq!(outcome.result.unwrap_err().code, 400);
        // Only 1 call -- no retry, no fallback
        assert_eq!(call_count.load(Ordering::Relaxed), 1);
        let recorded = attempts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].provider_name, "alpha");
        assert_eq!(recorded[0].status_code, 400);
    }

    #[tokio::test(start_paused = true)]
    async fn test_backoff_delays() {
        let candidates = vec![CandidateInfo {
            name: "alpha".to_string(),
        }];
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_inner = call_count.clone();
        let attempts: Arc<Mutex<Vec<AttemptRecord>>> = Arc::new(Mutex::new(Vec::new()));

        let start = tokio::time::Instant::now();

        let outcome: RetryOutcome<String, MockError> = retry_with_fallback(
            &candidates,
            attempts.clone(),
            |_info| {
                let cc = call_count_inner.clone();
                async move {
                    cc.fetch_add(1, Ordering::Relaxed);
                    Err(MockError { code: 503 })
                }
            },
        )
        .await;

        assert!(outcome.result.is_err());
        assert_eq!(call_count.load(Ordering::Relaxed), 3);

        // With start_paused = true, virtual time tracks sleep durations:
        // Attempt 1: immediate (0s elapsed)
        // Attempt 2: after 1s backoff (1s elapsed)
        // Attempt 3: after 2s backoff (3s total elapsed)
        let elapsed = start.elapsed();
        assert_eq!(elapsed, Duration::from_secs(3));
    }
}
