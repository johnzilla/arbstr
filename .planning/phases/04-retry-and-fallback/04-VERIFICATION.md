---
phase: 04-retry-and-fallback
verified: 2026-02-04T13:09:26Z
status: passed
score: 5/5 must-haves verified
re_verification: false
---

# Phase 4: Retry and Fallback Verification Report

**Phase Goal:** Failed requests are automatically retried and fall back to alternate providers without breaking API compatibility
**Verified:** 2026-02-04T13:09:26Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | A request that gets a 503 from the primary provider is retried with exponential backoff (up to 2 retries) and succeeds if the provider recovers | ✓ VERIFIED | `retry_with_fallback()` in `src/proxy/retry.rs` implements retry loop with `MAX_RETRIES=2`, `BACKOFF_DURATIONS=[1s, 2s, 4s]`, and `is_retryable()` checking for 5xx codes. Unit test `test_retry_then_success` verifies recovery after 503. |
| 2 | After retries are exhausted on the primary provider, the request is forwarded to the next cheapest provider offering the same model | ✓ VERIFIED | `retry_with_fallback()` takes `candidates` array, retries primary (index 0) up to MAX_RETRIES, then attempts fallback (index 1) once if exists. `select_candidates()` in `src/router/selector.rs` returns ordered cheapest-first list. Unit test `test_max_retries_then_fallback_success` verifies. |
| 3 | The x-arbstr-retries header shows the number of attempts and which providers were tried | ✓ VERIFIED | `format_retries_header()` in `src/proxy/retry.rs` formats "2/provider-alpha, 1/provider-beta". Handler in `src/proxy/handlers.rs:416-421` and `459-464` attaches header when `retries_header` is Some. Unit tests `test_format_retries_header_*` verify format. |
| 4 | Error responses through all retry and fallback paths remain valid OpenAI-compatible JSON with appropriate HTTP status codes | ✓ VERIFIED | Handler lines 449-466 show error path calls `outcome_err.error.into_response()` which uses existing `Error::into_response()` OpenAI-compatible impl. Timeout path (353-377) creates `Error::Provider`, converts via `into_response()`, then overrides status to 504. |
| 5 | Successful fallback requests are logged with the actual provider used, not the originally selected one | ✓ VERIFIED | Handler lines 384-403 show success logging uses `outcome.provider_name` from `RequestOutcome`, which comes from the actual provider that handled the request (primary or fallback). `send_to_provider()` sets provider_name in the outcome. |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/router/selector.rs` | `select_candidates()` method returning Vec<SelectedProvider> sorted cheapest-first | ✓ VERIFIED | Lines 86-129: method exists, sorts by `output_rate + base_fee`, deduplicates by name using HashSet. 5 unit tests pass (lines 343-478). |
| `src/router/mod.rs` | Re-export Router with select_candidates | ✓ VERIFIED | Router struct re-exported; select_candidates is public method on Router (no separate re-export needed). |
| `src/proxy/retry.rs` | Retry module with retry_with_fallback, AttemptRecord, CandidateInfo, HasStatusCode, RetryOutcome, is_retryable, format_retries_header | ✓ VERIFIED | All types and functions present. Lines 118-192: `retry_with_fallback()` with Arc<Mutex<Vec<AttemptRecord>>> parameter. 11 unit tests pass (lines 194-528). |
| `src/proxy/mod.rs` | `pub mod retry` declaration | ✓ VERIFIED | Line 7: `pub mod retry;` present. |
| `src/proxy/handlers.rs` | Retry-integrated handler with send_to_provider, timeout_at, Idempotency-Key, x-arbstr-retries | ✓ VERIFIED | Line 480-540: `send_to_provider()` with Idempotency-Key header (line 495). Lines 233-471: non-streaming path uses `select_candidates`, `retry_with_fallback`, `timeout_at` with 30s deadline. Lines 152-162: streaming path uses `execute_request` (no retry). |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| Router::select | Router::select_candidates | select() delegates to select_candidates().remove(0) | WIRED | `src/router/selector.rs:71-72` shows exact delegation pattern |
| chat_completions | select_candidates | Non-streaming path gets ordered candidate list | WIRED | `src/proxy/handlers.rs:236-238` calls `select_candidates` for non-streaming requests |
| chat_completions | retry_with_fallback | Wraps non-streaming request in retry loop | WIRED | `src/proxy/handlers.rs:299-308` calls `retry_with_fallback` with candidate_infos and closure |
| retry_with_fallback | is_retryable | Checks status code to decide retry vs fail | WIRED | `src/proxy/retry.rs:148` calls `is_retryable(err.status_code())` |
| retry_with_fallback | tokio::time::sleep | Backoff delay before each retry | WIRED | `src/proxy/retry.rs:140` calls `tokio::time::sleep(BACKOFF_DURATIONS[...])` |
| retry_with_fallback | Arc<Mutex<Vec<AttemptRecord>>> | Records attempts to shared vec that survives timeout | WIRED | `src/proxy/retry.rs:151-154` pushes to `attempts.lock().unwrap()`. Handler creates Arc before timeout (line 295), reads after (line 314). |
| send_to_provider | Idempotency-Key | Adds correlation ID as idempotency key | WIRED | `src/proxy/handlers.rs:495` sets header `.header("Idempotency-Key", correlation_id)` |
| chat_completions | timeout_at | 30-second deadline wrapping retry+fallback | WIRED | `src/proxy/handlers.rs:296-309` wraps retry_with_fallback in `timeout_at(deadline, ...)` |
| chat_completions | format_retries_header | Builds x-arbstr-retries header from attempt history | WIRED | `src/proxy/handlers.rs:315` calls `format_retries_header(&recorded_attempts)`, attached at lines 416-421 and 459-464 |
| Streaming path | execute_request | Streaming bypasses retry, uses single provider | WIRED | `src/proxy/handlers.rs:152-162` checks `is_streaming` and calls `execute_request` with correlation_id, no retry logic |

### Requirements Coverage

| Requirement | Status | Supporting Truths |
|-------------|--------|-------------------|
| RLBTY-01: Failed requests (429, 500, 502, 503, 504) retried with exponential backoff, max 2 retries | ✓ SATISFIED | Truth 1: Retry with backoff verified |
| RLBTY-02: After retries exhausted, falls back to next cheapest provider | ✓ SATISFIED | Truth 2: Fallback after primary exhausted verified |
| RLBTY-03: Router returns ordered list of candidate providers | ✓ SATISFIED | Artifact: select_candidates() verified with ordering tests |
| RLBTY-04: Retry/fallback metadata in x-arbstr-retries header | ✓ SATISFIED | Truth 3: Header format and attachment verified |
| RLBTY-05: Error responses remain OpenAI-compatible through all paths | ✓ SATISFIED | Truth 4: OpenAI-compatible error handling verified |

### Anti-Patterns Found

None detected. Code review findings:

| Category | Finding | Severity |
|----------|---------|----------|
| Quality | BACKOFF_DURATIONS uses [Duration; 3] with full sequence documented, though only first two used at runtime | ℹ️ Info |
| Quality | Dead code warnings addressed: removed select_cheapest/select_first, added #[allow(dead_code)] on default_strategy | ℹ️ Info |
| Quality | MockError test type properly implements Debug derive | ℹ️ Info |

### Human Verification Required

#### 1. Non-streaming Request Retry Behavior

**Test:** Start mock server with simulated 503 errors. Send non-streaming request via curl. Observe retry attempts in logs and x-arbstr-retries header in response.

**Expected:** 
- Logs show 3 attempts on primary provider with 1s and 2s delays
- If all fail, one fallback attempt to second provider
- Response includes `x-arbstr-retries: 3/provider-alpha` or `x-arbstr-retries: 3/provider-alpha, 1/provider-beta`

**Why human:** Requires observing time-based behavior (backoff delays) and inspecting logs for retry sequence. Automated tests verify logic, but human verification confirms end-to-end timing and observability.

#### 2. Streaming Request Fail-Fast

**Test:** Start mock server. Send streaming request (`"stream": true`) that fails. Observe logs.

**Expected:** 
- Single attempt only, no retries
- No x-arbstr-retries header
- Streaming responses fail immediately on error

**Why human:** Streaming behavior is explicitly different from non-streaming. Need to verify the path split works correctly in practice.

#### 3. Timeout After 30 Seconds

**Test:** Configure mock server with long delays. Send non-streaming request. Observe timeout behavior.

**Expected:**
- After 30 seconds, receive 504 Gateway Timeout response
- Response includes x-arbstr-retries header showing attempts made before timeout
- Error message mentions "retry budget exhausted"

**Why human:** Timeout behavior involves waiting 30 seconds and verifying the attempt history survives cancellation. Time-based behavior best verified manually.

#### 4. Idempotency-Key Header Sent Upstream

**Test:** Capture upstream requests (e.g., via provider logs or network inspection). Verify Idempotency-Key header present.

**Expected:**
- Each upstream request includes `Idempotency-Key: {correlation-id}`
- Same key used for all retry attempts of the same request

**Why human:** Requires inspecting actual HTTP requests sent to providers, which is external to the codebase. Code review confirms the header is set, but actual transmission needs verification.

---

## Verification Methodology

### Artifacts (Three-Level Check)

**Level 1: Existence**
- ✓ All 4 artifacts exist in codebase

**Level 2: Substantive**
- ✓ `src/router/selector.rs`: 480 lines, no stub patterns, exports select_candidates
- ✓ `src/proxy/retry.rs`: 529 lines, no stub patterns, exports all required types/functions
- ✓ `src/proxy/handlers.rs`: contains retry_with_fallback calls, send_to_provider function, timeout logic
- ✓ `src/proxy/mod.rs`: 8 lines total, declares retry module

**Level 3: Wired**
- ✓ `select_candidates`: Called by handler line 238, imported in Router
- ✓ `retry_with_fallback`: Called by handler line 301, imported from retry module
- ✓ `send_to_provider`: Called from retry closure line 306, from execute_request line 583
- ✓ All types (AttemptRecord, CandidateInfo, HasStatusCode) used in handler integration

### Key Links (Direct Wiring)

Verified 10 critical connections via grep and code inspection:
- ✓ select() → select_candidates delegation pattern present
- ✓ Non-streaming path uses select_candidates (streaming uses select)
- ✓ retry_with_fallback integration in non-streaming handler
- ✓ is_retryable check in retry loop
- ✓ tokio::time::sleep calls for backoff
- ✓ Arc<Mutex<Vec>> pattern for timeout-safe tracking
- ✓ Idempotency-Key header in send_to_provider
- ✓ timeout_at wrapping retry+fallback
- ✓ format_retries_header usage
- ✓ Streaming path bypasses retry (uses execute_request)

### Tests

**Unit Tests:**
- ✓ `cargo test` passes: 33 tests total
  - 11 router tests (including 5 new select_candidates tests)
  - 11 retry module tests
  - 11 other tests
- ✓ `cargo clippy -- -D warnings` clean
- ✓ `cargo build` successful

**Coverage Analysis:**
- Router candidate selection: 5 tests (ordering, dedup, delegation, filtering, errors)
- Retry logic: 11 tests (success, retry sequences, fallback, non-retryable, backoff timing)
- Integration: Manual verification required (see Human Verification section)

---

## Summary

### Phase Goal Achievement: ✓ VERIFIED

**Goal:** Failed requests are automatically retried and fall back to alternate providers without breaking API compatibility

**Evidence:**
1. Non-streaming requests use retry_with_fallback with 1s/2s backoff and up to 2 retries (Truth 1 ✓)
2. After primary retries exhaust, fallback to next candidate occurs once (Truth 2 ✓)
3. x-arbstr-retries header shows attempt history in "N/provider" format (Truth 3 ✓)
4. All error responses use OpenAI-compatible Error::into_response() (Truth 4 ✓)
5. Logging uses actual provider name from outcome, not original selection (Truth 5 ✓)

**Implementation Quality:**
- All 4 required artifacts exist and are substantive (not stubs)
- All 10 key links verified as wired correctly
- 33 automated tests pass with no clippy warnings
- Streaming/non-streaming path split correctly implemented
- Timeout handling preserves attempt history via Arc<Mutex> pattern
- Idempotency-Key header added for upstream deduplication

**Deviations:** None from plans. Two minor auto-fixes during implementation:
1. Dead code removal (select_cheapest, select_first) to satisfy clippy
2. Debug derive on test MockError type

**Human Verification:** 4 items flagged for manual testing (retry timing, streaming fail-fast, timeout behavior, upstream headers). These verify end-to-end behavior that automated tests cannot fully capture.

### Next Steps

Phase 4 is complete. All requirements (RLBTY-01 through RLBTY-05) are satisfied. Recommend:
1. Run human verification tests to confirm end-to-end behavior
2. Consider moving to v2 requirements (circuit breaker, per-provider timeouts)
3. Optional: Add integration tests that simulate provider failures

---

_Verified: 2026-02-04T13:09:26Z_
_Verifier: Claude (gsd-verifier)_
