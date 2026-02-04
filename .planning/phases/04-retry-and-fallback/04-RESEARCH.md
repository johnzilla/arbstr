# Phase 4: Retry and Fallback - Research

**Researched:** 2026-02-04
**Domain:** HTTP retry/fallback patterns in async Rust (Tokio + reqwest + axum)
**Confidence:** HIGH

## Summary

This phase adds automatic retry with exponential backoff and single-provider fallback to the existing `execute_request` flow in arbstr. The core architecture change is: (1) the Router must return an ordered candidate list instead of a single provider, and (2) a new retry/fallback loop wraps the existing provider-call logic, tracking attempt history for the `x-arbstr-retries` header.

The decisions lock this to a simple, deterministic retry pattern: fixed backoff (1s, 2s, 4s), max 2 retries on primary, one fallback provider with one shot, 30-second total deadline, non-streaming only. This simplicity means a hand-rolled retry loop using `tokio::time::sleep` and `tokio::time::timeout` is the right approach -- retry crate libraries (`tokio-retry2`, `backoff`, `reqwest-retry`) add dependency weight and API complexity for a loop that is approximately 30 lines of straightforward Rust. The fixed delays (no jitter, no randomization) and the fallback-to-different-provider logic do not map cleanly onto any retry crate's abstraction.

The existing `execute_request` function already separates provider selection from request forwarding and returns `Result<RequestOutcome, RequestError>`. The retry loop should wrap the forwarding portion (not the provider selection), attempting the request against the primary provider with retries, then against the fallback provider once if needed.

**Primary recommendation:** Hand-roll the retry loop with `tokio::time::sleep` + `tokio::time::timeout`, add `select_candidates` to the Router, and place the retry logic in a new `retry` module under `src/proxy/`.

## Standard Stack

No new crate dependencies are required. All retry/timeout functionality uses existing dependencies.

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio | 1.x (already in Cargo.toml) | `tokio::time::sleep` for backoff delays, `tokio::time::timeout` for 30s deadline | Already the async runtime; sleep/timeout are zero-cost abstractions |
| reqwest | 0.12 (already in Cargo.toml) | HTTP client for upstream requests | Already used for provider calls |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| wiremock | 0.6 (already in dev-dependencies) | Mock server for retry/fallback integration tests | Testing retry sequences with `up_to_n_times` and custom `Respond` impls |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-rolled retry loop | `tokio-retry2` 0.9 | Adds a dependency; its `RetryError::permanent`/`transient` model fits 4xx/5xx classification, but the fallback-to-different-provider logic would still need custom wrapping outside the crate. Net: more complexity for this use case. |
| Hand-rolled retry loop | `reqwest-retry` 0.9 via `reqwest-middleware` | Operates at the HTTP client middleware level; would retry transparently but cannot do provider fallback (different URL/auth per provider). Wrong abstraction level for this problem. |
| Hand-rolled retry loop | `backoff` crate | Similar to tokio-retry2; good for simple retry but does not model multi-provider fallback. |

**No `cargo add` needed.** All required functionality exists in the current dependency set.

## Architecture Patterns

### Recommended Project Structure
```
src/
├── proxy/
│   ├── handlers.rs   # chat_completions handler calls retry module
│   ├── retry.rs      # NEW: retry_with_fallback() function + AttemptRecord type
│   ├── server.rs     # No changes needed
│   ├── types.rs      # No changes needed
│   └── mod.rs        # Add `pub mod retry;`
└── router/
    └── selector.rs   # Add select_candidates() returning Vec<SelectedProvider>
```

### Pattern 1: Router Returns Ordered Candidate List
**What:** Add a `select_candidates` method to Router that returns `Vec<SelectedProvider>` sorted by cost (cheapest first), filtered by model and policy constraints. The existing `select` method can delegate to this.
**When to use:** Whenever fallback needs the next-cheapest same-model provider.
**Example:**
```rust
// In router/selector.rs
pub fn select_candidates(
    &self,
    model: &str,
    policy_name: Option<&str>,
    prompt: Option<&str>,
) -> Result<Vec<SelectedProvider>> {
    let policy = self.find_policy(policy_name, prompt);
    let mut candidates: Vec<&ProviderConfig> = self
        .providers
        .iter()
        .filter(|p| p.models.is_empty() || p.models.iter().any(|m| m == model))
        .collect();

    if candidates.is_empty() {
        return Err(Error::NoProviders { model: model.to_string() });
    }

    if let Some(policy) = &policy {
        candidates = self.apply_policy_constraints(candidates, policy, model)?;
    }

    // Sort by routing cost (output_rate + base_fee), cheapest first
    candidates.sort_by_key(|p| p.output_rate + p.base_fee);

    // Deduplicate by provider name (keep cheapest, which is first)
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<SelectedProvider> = candidates
        .into_iter()
        .filter(|p| seen.insert(p.name.clone()))
        .map(SelectedProvider::from)
        .collect();

    if unique.is_empty() {
        return Err(Error::NoPolicyMatch);
    }
    Ok(unique)
}

// Existing select() can delegate:
pub fn select(&self, model: &str, policy_name: Option<&str>, prompt: Option<&str>) -> Result<SelectedProvider> {
    self.select_candidates(model, policy_name, prompt)
        .map(|mut v| v.remove(0))
}
```

### Pattern 2: Retry Loop with Deadline
**What:** A standalone async function that wraps `tokio::time::timeout` around the entire retry+fallback chain, uses `tokio::time::sleep` for backoff delays, and tracks attempts in a `Vec<AttemptRecord>`.
**When to use:** For non-streaming requests after provider selection.
**Structure:**
```rust
// In proxy/retry.rs

/// Record of a single attempt for building x-arbstr-retries header
pub struct AttemptRecord {
    pub provider_name: String,
    pub attempt_number: u32, // 1-based within this provider
    pub status_code: u16,
}

/// Result of retry_with_fallback, either success or exhausted failure
pub struct RetryOutcome {
    pub result: std::result::Result<RequestOutcome, RequestError>,
    pub attempts: Vec<AttemptRecord>,
}

const BACKOFF_DURATIONS: [Duration; 2] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    // 4s would be third but we only have 2 retries
];
// Wait: 2 retries means delays are before attempt 2 and attempt 3
// Attempt 1: immediate, Attempt 2: wait 1s, Attempt 3: wait 2s
// Actually with 1s,2s,4s and max 2 retries:
// Attempt 1: immediate
// Attempt 2: wait 1s
// Attempt 3: wait 2s
// The "4s" in the spec is the third backoff slot but never used with max 2 retries

const MAX_RETRIES: u32 = 2;
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
```

### Pattern 3: Idempotency Key Header (IETF Draft Convention)
**What:** Send the correlation ID as `Idempotency-Key` header on upstream requests.
**When to use:** On every upstream request to enable provider-side deduplication on retries.
**Rationale:** The IETF draft RFC `draft-ietf-httpapi-idempotency-key-header-07` (published October 2025, active through April 2026) standardizes the `Idempotency-Key` header name. Stripe, PayPal, and others use this convention. The value is a UUID string -- the existing `x-arbstr-request-id` correlation ID is already a UUID v4 and serves this purpose perfectly.
**Example:**
```rust
// When building upstream request:
upstream_request = upstream_request
    .header("Idempotency-Key", &correlation_id);
```

### Pattern 4: `x-arbstr-retries` Header Construction
**What:** Build the header value from attempt records after the retry+fallback loop completes.
**Format:** `"2/provider-alpha, 1/provider-beta"` -- count of failed attempts per provider.
**When:** Only include providers where attempts were made. The count is the number of *failed* attempts for that provider (not the successful one if it succeeded).
**Example:**
```rust
fn format_retries_header(attempts: &[AttemptRecord]) -> Option<String> {
    // Group failed attempts by provider, count per provider
    let mut counts: Vec<(String, u32)> = Vec::new();
    for attempt in attempts {
        if let Some(entry) = counts.iter_mut().find(|(name, _)| *name == attempt.provider_name) {
            entry.1 += 1;
        } else {
            counts.push((attempt.provider_name.clone(), 1));
        }
    }
    if counts.is_empty() {
        return None;
    }
    Some(counts.iter().map(|(name, count)| format!("{}/{}", count, name)).collect::<Vec<_>>().join(", "))
}
```

### Pattern 5: `timeout_at` for Shared Deadline
**What:** Use `tokio::time::timeout_at(deadline, ...)` to wrap the entire retry+fallback operation under a single 30-second deadline, rather than computing remaining time at each step.
**Why:** Cleaner than `timeout(remaining, ...)` because the deadline is computed once and shared across all attempts. If the deadline expires mid-retry, the current attempt is cancelled and the timeout error propagates.
**Example:**
```rust
use tokio::time::{timeout_at, Instant, Duration};

let deadline = Instant::now() + Duration::from_secs(30);

let outcome = timeout_at(deadline, async {
    // entire retry + fallback loop here
    retry_with_fallback(candidates, state, request, ...).await
}).await;

match outcome {
    Ok(retry_outcome) => { /* use retry_outcome */ }
    Err(_elapsed) => { /* 30s deadline exceeded, return timeout error */ }
}
```

### Anti-Patterns to Avoid
- **Retrying inside execute_request:** The retry logic should wrap the request-sending portion, not be embedded inside `execute_request`. Keep `execute_request` as the single-attempt function.
- **Retrying streaming requests:** The context explicitly forbids this. Streaming requests skip the retry path entirely and fail fast.
- **Using reqwest-middleware for retry:** This operates at the wrong level (same URL on each retry). We need to switch providers/URLs on fallback.
- **Retrying 4xx errors:** These are permanent failures. Only 5xx (500, 502, 503, 504) trigger retry.
- **Sleeping after the last failed attempt:** Only sleep *before* the next attempt, not after a final failure.

## Don't Hand-Roll

Problems that look simple but have existing solutions:

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Backoff delay timing | Custom timer management | `tokio::time::sleep(Duration)` | Exact, cancellation-safe, zero-overhead |
| Overall timeout | Manual elapsed tracking | `tokio::time::timeout_at(deadline, future)` | Automatically cancels inner future on expiry, no drift |
| Mock HTTP servers for testing | Custom TCP listeners | `wiremock` crate (already in dev-deps) | Handles port binding, request matching, response templating |
| UUID generation for idempotency keys | Custom ID gen | `uuid::Uuid::new_v4()` (already in deps) | Already used for correlation IDs |

**Key insight:** The retry *logic* itself (the loop, the condition checks, the provider switching) is domain-specific to arbstr's multi-provider model and is best hand-rolled. The *primitives* (sleep, timeout, mock servers) already exist in the dependency tree and should be used as-is.

## Common Pitfalls

### Pitfall 1: Timeout Budget Not Shared Across Retries
**What goes wrong:** Each retry gets its own 30s timeout instead of sharing a 30s total budget. A request with 3 retries + 1 fallback could take up to 120 seconds.
**Why it happens:** Using `tokio::time::timeout(Duration::from_secs(30), single_attempt())` inside the loop instead of wrapping the entire loop.
**How to avoid:** Use `tokio::time::timeout_at` with a single deadline computed once, wrapping the entire retry+fallback loop.
**Warning signs:** Tests pass individually but real-world requests hang for minutes.

### Pitfall 2: Sleeping After Final Attempt
**What goes wrong:** The retry loop sleeps before returning the final error, wasting time.
**Why it happens:** Backoff sleep placed at the end of the loop body rather than at the beginning of the next iteration.
**How to avoid:** Structure the loop so sleep happens *before* the next attempt, not after a failed attempt.
**Warning signs:** Tests show unexpected additional latency on final failures.

### Pitfall 3: Retrying Non-5xx Errors
**What goes wrong:** Connection errors from reqwest (DNS failure, connection refused) return as `reqwest::Error` rather than an HTTP status. These get treated as retryable when they might indicate a permanently unavailable provider.
**Why it happens:** Only checking `status.is_server_error()` but not handling the case where no HTTP response is received at all.
**How to avoid:** Network-level errors (connection timeout, DNS failure) from reqwest should also be treated as retryable since they indicate transient infrastructure issues. The key decision: reqwest errors that produce no HTTP status code at all (connection refused, timeout, DNS) should be retried the same as 5xx, because they are transient. The status code stored in `AttemptRecord` should be 502 for these cases (matching the existing `RequestError` behavior in `execute_request`).
**Warning signs:** Tests with mock server down never trigger retry.

### Pitfall 4: Fallback to Same Provider
**What goes wrong:** If the same provider appears twice in the config (e.g., different rate tiers), fallback picks the same provider name.
**Why it happens:** Candidate list not deduplicated by provider name.
**How to avoid:** `select_candidates` must deduplicate by provider name. The context decision explicitly says "Fallback must be a different provider name."
**Warning signs:** Fallback tests show the same provider being used.

### Pitfall 5: Logging Retry Attempts Instead of Final Outcome
**What goes wrong:** Each retry attempt logs a separate row in the database, creating noise and incorrect cost tracking.
**Why it happens:** Logging placed inside the retry loop.
**How to avoid:** Log only the final outcome after the retry+fallback loop completes. The `provider` field in the log should show the provider that actually handled the request (which may be the fallback provider).
**Warning signs:** Database has multiple rows per correlation_id.

### Pitfall 6: reqwest Per-Request Timeout Interfering with Retry Deadline
**What goes wrong:** The global reqwest client timeout (currently 120s in server.rs) is longer than the 30s retry deadline. If a provider hangs, `timeout_at` cancels the future correctly, but the reqwest connection may linger.
**Why it happens:** The reqwest client was built with a 120s timeout before retry was introduced.
**How to avoid:** This is not actually a problem because `timeout_at` cancels the inner future (dropping the reqwest future closes the connection). However, consider reducing the per-request timeout on the reqwest client to 15-20 seconds so individual provider calls don't consume the entire 30s budget. This is a tuning concern, not a correctness bug.
**Warning signs:** A single hanging provider exhausts the entire retry budget with one attempt.

## Code Examples

### Example 1: Retry Loop Core Structure
```rust
// src/proxy/retry.rs

use std::time::Duration;
use tokio::time::{sleep, timeout_at, Instant};

use super::handlers::{RequestOutcome, RequestError};
use crate::router::SelectedProvider;

/// Fixed backoff durations: 1s before retry 1, 2s before retry 2
const BACKOFF_SECS: [u64; 2] = [1, 2];
const MAX_RETRIES: u32 = 2;

/// Whether an error status code should trigger a retry
fn is_retryable(status_code: u16) -> bool {
    matches!(status_code, 500 | 502 | 503 | 504)
}

/// Record of a failed attempt
pub struct AttemptRecord {
    pub provider_name: String,
    pub status_code: u16,
}

/// Outcome of the full retry+fallback sequence
pub struct RetryOutcome {
    pub result: Result<RequestOutcome, RequestError>,
    pub attempts: Vec<AttemptRecord>,
}

/// Execute a request with retry on the primary provider and fallback to the next candidate.
///
/// - Up to MAX_RETRIES retries on the primary (first candidate)
/// - On exhaustion, one shot on the fallback (second candidate) if available
/// - Returns the attempt history for x-arbstr-retries header
pub async fn retry_with_fallback(
    candidates: Vec<SelectedProvider>,
    send_request: impl Fn(&SelectedProvider) -> /* future returning Result<RequestOutcome, RequestError> */,
) -> RetryOutcome {
    let mut attempts: Vec<AttemptRecord> = Vec::new();

    // Primary provider: up to MAX_RETRIES+1 total attempts
    let primary = &candidates[0];
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            sleep(Duration::from_secs(BACKOFF_SECS[(attempt - 1) as usize])).await;
        }

        match send_request(primary).await {
            Ok(outcome) => return RetryOutcome { result: Ok(outcome), attempts },
            Err(err) => {
                let retryable = is_retryable(err.status_code);
                attempts.push(AttemptRecord {
                    provider_name: primary.name.clone(),
                    status_code: err.status_code,
                });
                if !retryable || attempt == MAX_RETRIES {
                    // Not retryable or exhausted retries
                    if !retryable {
                        return RetryOutcome { result: Err(err), attempts };
                    }
                    // Fall through to fallback
                    break;
                }
                // Otherwise loop continues with backoff
            }
        }
    }

    // Fallback: one shot on second candidate (if available)
    if candidates.len() > 1 {
        let fallback = &candidates[1];
        match send_request(fallback).await {
            Ok(outcome) => return RetryOutcome { result: Ok(outcome), attempts },
            Err(err) => {
                attempts.push(AttemptRecord {
                    provider_name: fallback.name.clone(),
                    status_code: err.status_code,
                });
                return RetryOutcome { result: Err(err), attempts };
            }
        }
    }

    // No fallback available, return last error
    RetryOutcome {
        result: Err(/* last error from primary */),
        attempts,
    }
}
```

### Example 2: Handler Integration with timeout_at
```rust
// In proxy/handlers.rs - modified chat_completions handler

// For non-streaming requests:
if !is_streaming {
    let candidates = state.router.select_candidates(&request.model, policy_name.as_deref(), user_prompt)?;
    let deadline = Instant::now() + Duration::from_secs(30);

    let retry_result = timeout_at(deadline, retry_with_fallback(
        candidates,
        |provider| send_to_provider(state, request, provider, &correlation_id),
    )).await;

    match retry_result {
        Ok(outcome) => {
            // outcome.result is the RequestOutcome or RequestError
            // outcome.attempts is the attempt history for x-arbstr-retries
        }
        Err(_elapsed) => {
            // 30s deadline exceeded -- return 504 Gateway Timeout
        }
    }
} else {
    // Streaming: single attempt, no retry (existing behavior)
}
```

### Example 3: x-arbstr-retries Header Formatting
```rust
/// Format attempt records into the x-arbstr-retries header value.
///
/// Format: "2/provider-alpha, 1/provider-beta"
/// Only includes failed attempts. Returns None if no retries occurred.
pub fn format_retries_header(attempts: &[AttemptRecord]) -> Option<String> {
    if attempts.is_empty() {
        return None;
    }
    // Preserve order of first appearance
    let mut counts: Vec<(&str, u32)> = Vec::new();
    for attempt in attempts {
        if let Some(entry) = counts.iter_mut().find(|(name, _)| *name == attempt.provider_name) {
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
```

### Example 4: Wiremock Test for Retry Sequence
```rust
use wiremock::{MockServer, Mock, ResponseTemplate, Respond, Request};
use wiremock::matchers::{method, path};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Custom responder that returns different status codes in sequence
struct SequentialResponder {
    responses: Vec<u16>,
    call_count: AtomicUsize,
}

impl SequentialResponder {
    fn new(status_codes: Vec<u16>) -> Self {
        Self {
            responses: status_codes,
            call_count: AtomicUsize::new(0),
        }
    }
}

impl Respond for SequentialResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let idx = self.call_count.fetch_add(1, Ordering::Relaxed);
        let status = self.responses.get(idx).copied().unwrap_or(500);
        if status == 200 {
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({
                    "id": "chatcmpl-test",
                    "object": "chat.completion",
                    "created": 1234567890,
                    "model": "gpt-4o",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "hello"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                }))
        } else {
            ResponseTemplate::new(status)
                .set_body_json(serde_json::json!({"error": {"message": "server error", "type": "server_error"}}))
        }
    }
}

#[tokio::test]
async fn test_retry_then_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(SequentialResponder::new(vec![500, 500, 200]))
        .mount(&server)
        .await;
    // ... set up client pointing at server.uri(), verify 200 response with x-arbstr-retries: "2/provider-name"
}
```

### Example 5: Idempotency Key in Upstream Request
```rust
// When building the upstream request, add the idempotency key:
let mut upstream_request = state
    .http_client
    .post(&upstream_url)
    .header(header::CONTENT_TYPE, "application/json")
    .header("Idempotency-Key", correlation_id)
    .json(request);
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Custom `X-Idempotency-Key` header | `Idempotency-Key` (IETF draft-07) | Oct 2025 | Standard header name, MDN documented |
| `tokio-retry` (original, unmaintained) | `tokio-retry2` 0.9 or hand-rolled | 2024 | tokio-retry2 adds conditional retry; hand-rolling is fine for simple cases |
| Per-attempt timeout | Shared deadline with `timeout_at` | Always been available in tokio | Ensures total wall-clock budget is respected |

**Deprecated/outdated:**
- `tokio-retry` 0.3 (original crate): Unmaintained, superseded by `tokio-retry2`. Neither is needed for this use case.
- `X-Idempotency-Key` / `X-Request-Id` as idempotency: Use the IETF standard `Idempotency-Key` header name instead.

## Open Questions

1. **reqwest client per-request timeout vs retry budget**
   - What we know: The current reqwest client has a 120s timeout. The retry budget is 30s total. A single hanging provider could consume the entire 30s.
   - What's unclear: Whether we should create a separate reqwest client with a shorter per-request timeout (e.g., 10s) for retry-eligible requests, or rely on `timeout_at` to cancel them.
   - Recommendation: Rely on `timeout_at` for now. The `timeout_at` wrapper will cancel the inner future if it exceeds the deadline. This is simpler and avoids managing multiple HTTP clients. If individual request timeouts become important, that can be tuned later (deferred RLBTY-08).

2. **RequestOutcome/RequestError visibility**
   - What we know: `RequestOutcome` and `RequestError` are currently `struct` (not `pub`) in `handlers.rs`. The retry module will need access to them.
   - What's unclear: Whether to make them `pub` or to extract them to a shared types location.
   - Recommendation: Make them `pub(crate)` in handlers.rs, or move them to `proxy/types.rs`. Either works; the planner should decide based on module organization preference.

3. **Closure vs function for send_request**
   - What we know: The retry loop needs to call "send request to provider X" as a reusable operation. Currently this logic lives inline in `execute_request`.
   - What's unclear: Exact function signature -- needs access to `AppState`, `ChatCompletionRequest`, `SelectedProvider`, and `correlation_id`.
   - Recommendation: Extract a `send_to_provider` async function that takes these parameters and returns `Result<RequestOutcome, RequestError>`. The retry loop calls this function with different providers. This avoids complex closure lifetime issues.

## Sources

### Primary (HIGH confidence)
- Codebase inspection: `src/proxy/handlers.rs`, `src/router/selector.rs`, `src/proxy/server.rs`, `src/config.rs` -- direct reading of current implementation
- [tokio::time::timeout official docs](https://docs.rs/tokio/latest/tokio/time/fn.timeout.html) -- timeout API, Elapsed error type
- [tokio::time::timeout_at official docs](https://docs.rs/tokio/latest/tokio/time/fn.timeout_at.html) -- deadline-based timeout pattern
- [IETF draft-ietf-httpapi-idempotency-key-header-07](https://datatracker.ietf.org/doc/draft-ietf-httpapi-idempotency-key-header/) -- Idempotency-Key header standard (Oct 2025)
- [MDN Idempotency-Key header](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Idempotency-Key) -- Browser/standard documentation

### Secondary (MEDIUM confidence)
- [tokio-retry2 docs](https://docs.rs/tokio-retry2) -- RetryError::permanent/transient pattern (verified via docs.rs)
- [reqwest-retry docs](https://docs.rs/reqwest-retry) -- RetryTransientMiddleware (verified via docs.rs)
- [wiremock-rs docs](https://docs.rs/wiremock/) -- Respond trait for sequential responses (verified via docs.rs)
- [reqwest retries blog by seanmonstar](https://seanmonstar.com/blog/reqwest-retries/) -- reqwest built-in retry design philosophy

### Tertiary (LOW confidence)
- [Retry with exponential backoff blog](https://oneuptime.com/blog/post/2026-01-07-rust-retry-exponential-backoff/view) -- General patterns (Jan 2026, used for cross-reference only)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- No new dependencies; all primitives (sleep, timeout, wiremock) are in existing Cargo.toml and verified via official docs
- Architecture: HIGH -- Based on direct reading of current codebase; Router changes and retry module placement are straightforward extensions of existing patterns
- Pitfalls: HIGH -- Based on common async Rust patterns and direct analysis of the existing timeout/logging code
- Code examples: MEDIUM -- Patterns are sound but exact signatures will depend on planner decisions about type visibility and module organization

**Research date:** 2026-02-04
**Valid until:** 2026-03-06 (30 days -- stable domain, no fast-moving dependencies)
