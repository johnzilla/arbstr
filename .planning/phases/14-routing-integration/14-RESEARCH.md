# Phase 14: Routing Integration - Research

**Researched:** 2026-02-16
**Domain:** Handler-level circuit breaker integration for provider filtering, fail-fast 503, and outcome recording
**Confidence:** HIGH

## Summary

Phase 14 wires the Phase 13 circuit breaker state machine into the existing request handlers. The integration points are: (1) filtering candidates before the retry loop in the non-streaming path, (2) checking circuit state before provider selection in the streaming path, (3) returning 503 when all candidate circuits are open, and (4) recording success/failure outcomes after requests complete.

The locked decision is "handler-level integration (not router or middleware)." This means the circuit breaker logic lives in `handlers.rs`, not inside the `Router` or as axum middleware. The handler calls `acquire_permit` on the circuit breaker registry before forwarding to a provider, and records outcomes after the response is received. The existing `retry_with_fallback` function in `retry.rs` does not need modification -- it already takes a filtered candidate list.

**Primary recommendation:** Modify `src/proxy/handlers.rs` to add circuit breaker filtering before provider selection (non-streaming) and before `execute_request` (streaming). Add outcome recording after request completion in both paths. The non-streaming path filters candidates before the retry loop; the streaming path's background task records outcomes after stream completion. Add a new `Error::CircuitOpen` variant for the 503 fail-fast response.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|-----------------|
| RTG-01 | Router skips providers with open circuits during candidate selection | Handler filters `select_candidates()` results via `acquire_permit()` before passing to `retry_with_fallback`. Candidates whose circuits are open are removed from the list. Probe permit handling determines which candidate acts as the probe request. |
| RTG-02 | When all providers for a model have open circuits, return 503 fail-fast | After filtering, if no candidates remain, handler returns `Error::CircuitOpen` which maps to HTTP 503 Service Unavailable. No provider requests are attempted. |
| RTG-03 | Non-streaming handler records success/failure outcomes to circuit breaker after retry | After `retry_with_fallback` completes, the handler calls `record_success` or `record_failure` on the circuit breaker registry based on the outcome. Failure classification: 5xx status codes and reqwest transport errors (status_code 502 in current code) are recorded as failures; 4xx are ignored. Probe results use `ProbeGuard.success()` / `ProbeGuard.failure()`. |
| RTG-04 | Streaming handler records outcomes in spawned background task after stream completes | Inside the existing `tokio::spawn` block in `handle_streaming_response`, after stream completion (after the `while let Some(chunk_result)` loop), the handler records the outcome to the circuit breaker. Success = 2xx initial response + stream received (already ensured by reaching `handle_streaming_response`). Per locked decision: "if 2xx is received and streaming begins, it counts as success even if the stream fails mid-way." |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| (none new) | - | All dependencies already in Cargo.toml | Phase 14 is pure integration -- no new crates needed |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `CircuitBreakerRegistry` | (Phase 13) | `acquire_permit`, `record_success`, `record_failure` | Every request path in handlers.rs |
| `ProbeGuard` | (Phase 13) | RAII probe lifecycle for half-open probes | When `acquire_permit` returns `PermitType::Probe` |
| `PermitType` | (Phase 13) | Distinguish normal vs probe permits | Handler dispatches differently for probe requests |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Handler-level integration | Router-level filtering | Router has no access to AppState.circuit_breakers; would require threading Arc through Router. Handler already has full state access. |
| Handler-level integration | Axum middleware | Middleware cannot inspect the parsed request body (model name) needed for candidate selection. Would require double-parse. |
| Filtering before retry loop | Filtering inside retry closure | Would cause the retry loop to re-check circuits on each attempt, potentially amplifying retries into open circuits. Pre-filtering is cleaner. |

## Architecture Patterns

### Existing Code Structure (Files to Modify)

```
src/proxy/
  handlers.rs    # PRIMARY: Add circuit filtering + outcome recording
  server.rs      # AppState already has circuit_breakers: Arc<CircuitBreakerRegistry>
  retry.rs       # NO CHANGES needed -- already takes pre-filtered candidates
  circuit_breaker.rs  # NO CHANGES needed -- API surface complete from Phase 13
src/error.rs     # Add Error::CircuitOpen variant for 503
```

### Pattern 1: Pre-Retry Circuit Filtering (Non-Streaming Path)

**What:** Filter candidates from `select_candidates()` through `acquire_permit()` before passing to `retry_with_fallback`. This is the core of RTG-01.

**When to use:** Non-streaming path in `chat_completions()`, between `select_candidates()` and `retry_with_fallback()`.

**Current flow (handlers.rs lines 238-312):**
```rust
// 1. Get ordered candidates
let candidates = state.router.select_candidates(&request.model, ...)?;
// 2. Build CandidateInfo list
let candidate_infos: Vec<CandidateInfo> = ...;
// 3. Run retry+fallback
let timeout_result = timeout_at(deadline, retry_with_fallback(&candidate_infos, ...)).await;
```

**New flow:**
```rust
// 1. Get ordered candidates (unchanged)
let candidates = state.router.select_candidates(&request.model, ...)?;

// 2. Filter through circuit breaker
let mut filtered_candidates = Vec::new();
let mut probe_guard: Option<ProbeGuard> = None;
for candidate in &candidates {
    match state.circuit_breakers.acquire_permit(&candidate.name).await {
        Ok(PermitType::Normal) => {
            filtered_candidates.push(candidate.clone());
        }
        Ok(PermitType::Probe) => {
            // This candidate gets a probe request -- add to front
            probe_guard = Some(ProbeGuard::new(&state.circuit_breakers, candidate.name.clone()));
            filtered_candidates.insert(0, candidate.clone());
        }
        Err(_circuit_open) => {
            // Skip this provider -- circuit is open
            tracing::debug!(provider = %candidate.name, "Skipping provider: circuit open");
        }
    }
}

// 3. Check if any candidates remain (RTG-02)
if filtered_candidates.is_empty() {
    // All circuits open -- 503 fail-fast
    return Err(Error::CircuitOpen { model: model.clone() });
}

// 4. Build CandidateInfo and run retry (unchanged structure)
let candidate_infos: Vec<CandidateInfo> = ...;
let timeout_result = timeout_at(deadline, retry_with_fallback(&candidate_infos, ...)).await;

// 5. Record outcome to circuit breaker (RTG-03)
// After retry completes, record success/failure for the provider that handled the request
```

**Key insight:** The probe guard's lifetime must span the entire retry+fallback attempt. If a probe candidate succeeds, call `probe_guard.success()`. If it fails, call `probe_guard.failure()`. If retry times out, ProbeGuard's Drop impl handles it (records failure).

### Pattern 2: Streaming Path Circuit Check

**What:** Check circuit state before executing the streaming request. Record outcome in the background task.

**When to use:** Streaming path in `chat_completions()`.

**Current flow (handlers.rs lines 154-233):**
```rust
if is_streaming {
    let result = execute_request(&state, &request, ...).await;
    // ...log and return
}
```

**New flow:**
```rust
if is_streaming {
    // execute_request internally calls select() which picks one provider.
    // We need to acquire_permit before the actual send.
    // Option A: Check circuit inside execute_request (requires passing circuit_breakers).
    // Option B: Select provider first, check circuit, then send directly.
    //
    // Decision: Modify execute_request to acquire a permit before sending.
    // Or: inline the logic here -- select candidate, check permit, send.

    let result = execute_request(&state, &request, ...).await;

    // Record outcome -- but streaming records in background task
    // The initial HTTP response status determines circuit outcome per CONTEXT.md:
    // "if 2xx is received and streaming begins, it counts as success"
}
```

### Pattern 3: Outcome Recording (Non-Streaming)

**What:** After retry completes, record success/failure outcomes to circuit breaker for each attempted provider.

**When to use:** After `retry_with_fallback` returns in the non-streaming path.

**Failure classification (from CONTEXT.md locked decisions):**
- **Counts as failure (record_failure):** HTTP 5xx responses (status 500, 502, 503, 504), request timeouts (the reqwest transport error maps to status_code 502 in `send_to_provider`)
- **Does NOT count:** HTTP 4xx (including 429), network errors (connection refused, DNS, TLS)
- **Counts as success (record_success):** HTTP 2xx response

**Implementation approach:**
```rust
// After retry_with_fallback returns:
match &timeout_result {
    Ok(retry_outcome) => {
        match &retry_outcome.result {
            Ok(outcome) => {
                // Success: record for the winning provider
                state.circuit_breakers.record_success(&outcome.provider_name);
                // If this was a probe, resolve the probe guard
                if let Some(guard) = probe_guard.take() {
                    if guard_provider == outcome.provider_name {
                        guard.success();
                    }
                }
            }
            Err(outcome_err) => {
                // Failure: the retry module already exhausted retries
                // Record failure for each attempted provider using attempt records
                // Only record 5xx as circuit breaker failures
                for attempt in &recorded_attempts {
                    if is_circuit_failure(attempt.status_code) {
                        state.circuit_breakers.record_failure(
                            &attempt.provider_name,
                            &classify_error_type(attempt.status_code),
                            &format!("HTTP {}", attempt.status_code),
                        );
                    }
                }
                // If this was a probe, resolve as failure
                if let Some(guard) = probe_guard.take() {
                    guard.failure("5xx", "probe request failed");
                }
            }
        }
    }
    Err(_timeout) => {
        // Timeout: record as failure for attempted providers
        // ...similar to above
    }
}
```

### Pattern 4: Streaming Outcome Recording (Background Task)

**What:** Record circuit breaker outcome in the existing `tokio::spawn` block.

**When to use:** After stream completion in `handle_streaming_response`.

**Key insight from CONTEXT.md:** "Streaming: only the initial HTTP response status matters -- if 2xx is received and streaming begins, it counts as success even if the stream fails mid-way."

This means the outcome can be recorded immediately when the streaming response is returned successfully from `send_to_provider`. The background task does NOT need to wait for stream completion to record the circuit outcome.

**Two options:**
1. Record success immediately after `send_to_provider` returns Ok (before spawning background task)
2. Record in background task after stream ends

**Recommendation:** Option 1 -- record immediately. Per the locked decision, a 2xx initial response = success regardless of stream outcome. Recording immediately is simpler and matches the decision. The background task only needs to handle the DB UPDATE for tokens/cost/duration.

However, RTG-04 specifically says "Streaming handler records outcomes in spawned background task after stream completes." This implies the recording should happen in the background task. The reconciliation: the initial 2xx is already confirmed by reaching `handle_streaming_response` (the error path in `send_to_provider` already handles non-success statuses). The background task records the confirmed success.

**Practical approach:** Record `record_success` for the provider immediately after `send_to_provider` returns `Ok` in the streaming path. This is technically "after stream completes" from the circuit breaker's perspective (the initial HTTP response is the circuit-relevant event). The background task's stream completion is irrelevant to circuit state.

**Alternative practical approach (matching RTG-04 wording):** Pass `circuit_breakers` Arc into the background task closure, record `record_success` at the end of the stream processing. This is slightly more literal but functionally equivalent because the initial 2xx already determined the outcome.

### Anti-Patterns to Avoid

- **Filtering inside retry closure:** Would re-check circuits on each retry attempt. The decision says "Filter candidates BEFORE retry loop (prevents retry storm amplification)."
- **Modifying Router to know about circuits:** Router is a pure selection algorithm. Circuit state is handler-level state. Keep separation.
- **Recording failures for 4xx responses:** Per locked decision, 4xx does NOT trip the circuit. Only 5xx and timeouts.
- **Holding Mutex across await:** The circuit breaker's `acquire_permit` already handles this correctly (drops all locks before awaiting). The handler must not introduce new lock-across-await patterns.
- **Forgetting ProbeGuard on all exit paths:** If acquire_permit returns Probe, a ProbeGuard must be created and resolved on every path (success, failure, timeout, early return). The RAII Drop impl handles the forgot-to-resolve case, but explicit resolution is preferred.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Circuit state checking | Custom check logic | `CircuitBreakerRegistry::acquire_permit()` | Handles Closed/Open/HalfOpen/WaitForProbe transitions automatically |
| Probe lifecycle | Manual probe_in_flight management | `ProbeGuard` RAII | Drop impl prevents stuck probes even on panics/early returns |
| Outcome classification | Complex match on reqwest error types | Check `status_code` field on `RequestError` / `AttemptRecord` | All error paths already map to a status_code (502 for transport errors, actual code for HTTP errors) |
| 503 response format | Manual JSON construction | `Error::CircuitOpen` variant + `IntoResponse` | Matches existing OpenAI-compatible error format pattern |

**Key insight:** All the complex circuit breaker logic is already built in Phase 13. Phase 14 is pure integration glue code -- calling `acquire_permit`, filtering, and calling `record_success`/`record_failure` at the right places.

## Common Pitfalls

### Pitfall 1: ProbeGuard Lifetime Across Timeout

**What goes wrong:** The non-streaming path wraps `retry_with_fallback` in `timeout_at`. If the timeout fires, the future is cancelled. If a ProbeGuard was created for a probe candidate, it must still be resolved.

**Why it happens:** `timeout_at` drops the inner future on cancellation. If ProbeGuard is created inside the future, its Drop runs and records failure. If it's created outside (before the timeout), it survives.

**How to avoid:** Create the ProbeGuard before the `timeout_at` call. Place it in an `Option<ProbeGuard>` that outlives the timeout. On timeout, ProbeGuard's Drop will correctly record failure. On success/failure, explicitly resolve it.

**Warning signs:** Tests where timeout leaves a provider's circuit in unexpected state.

### Pitfall 2: Failure Classification Mismatch

**What goes wrong:** Recording 4xx responses as circuit breaker failures, or ignoring transport errors that should count.

**Why it happens:** The `send_to_provider` function maps reqwest transport errors to `status_code: 502`, which IS a retryable code AND should trip the circuit. HTTP 4xx from the provider has `status_code: 4xx` and should NOT trip the circuit.

**How to avoid:** Use a helper function `is_circuit_failure(status_code: u16) -> bool` that returns true for 5xx only (same set as `is_retryable` minus any future changes). Reuse or align with `retry::is_retryable()`.

**Warning signs:** Circuits tripping on 401/403/429 errors.

### Pitfall 3: Recording Per-Attempt vs Per-Request

**What goes wrong:** Only recording the final outcome, not per-attempt outcomes. If provider A fails 3 times then provider B succeeds, provider A should have 3 failures recorded but provider B should have 1 success.

**Why it happens:** The retry module tracks `AttemptRecord` but the circuit breaker needs per-attempt recording.

**How to avoid:** There are two approaches:
1. Record all failed attempts from the `attempts` Vec after retry completes (bulk recording)
2. Record inside the retry closure on each attempt

**Recommended:** Option 1 (bulk recording after retry). The retry module already records all failed attempts in the shared `Arc<Mutex<Vec<AttemptRecord>>>`. After retry completes, iterate through attempts and record each failure. Then record the final success (if any). This keeps the retry module unchanged.

**Warning signs:** Provider A's failure count not incrementing despite repeated 5xx responses.

### Pitfall 4: Streaming Path Has No Retry

**What goes wrong:** Trying to add circuit filtering to the streaming path the same way as non-streaming, with retry and fallback.

**Why it happens:** The streaming path currently does not use retry_with_fallback. It calls `execute_request` which calls `select` (single provider) then `send_to_provider`.

**How to avoid:** The streaming path only needs:
1. Select a provider
2. Check circuit state for that provider (acquire_permit)
3. If open, try next candidate or fail-fast 503
4. If closed/probe, proceed with request
5. Record outcome

The streaming path does NOT need the full pre-filtering loop since it's currently single-provider. But for consistency, it should acquire_permit before sending.

**Warning signs:** Streaming requests going to providers with open circuits.

### Pitfall 5: Probe Candidate Ordering

**What goes wrong:** A probe request gets placed as the fallback candidate, meaning it's tried last after primary retries, which wastes the probe opportunity.

**Why it happens:** Candidates are sorted by cost. The probe candidate might not be the cheapest.

**How to avoid:** When a probe permit is acquired for a candidate, that candidate should be the first attempted (it IS the probe). Other candidates are tried as fallback. Insert probe candidate at front of the filtered list.

**Warning signs:** Probe requests never actually running because primary retries always succeed first.

### Pitfall 6: acquire_permit is Async (Blocks on Half-Open Wait)

**What goes wrong:** Calling `acquire_permit` in a loop for all candidates sequentially can block on a half-open probe wait, delaying the entire filtering phase.

**Why it happens:** `acquire_permit` is async -- when a probe is in-flight, it waits for the probe result.

**How to avoid:** Accept the sequential behavior. In practice, at most one provider will be in half-open state. The wait is bounded by the probe request duration. This is preferable to the complexity of concurrent filtering.

**Alternative:** If blocking becomes a problem in the future, use `tokio::select!` with a timeout per acquire_permit call. But this is premature optimization.

**Warning signs:** Slow request routing when a provider is in half-open state.

## Code Examples

### Error Variant for 503 Fail-Fast

```rust
// In src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // ... existing variants ...

    #[error("All providers have open circuits for model '{model}'")]
    CircuitOpen { model: String },
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            // ... existing matches ...
            Error::CircuitOpen { .. } => (StatusCode::SERVICE_UNAVAILABLE, self.to_string()),
        };
        // ... existing body format ...
    }
}
```

### Helper: Classify Error for Circuit Breaker

```rust
// In src/proxy/handlers.rs

/// Whether an HTTP status code should be recorded as a circuit breaker failure.
///
/// Returns true for 5xx server errors (same as retryable errors).
/// Returns false for 4xx client errors and all other codes.
fn is_circuit_failure(status_code: u16) -> bool {
    (500..600).contains(&status_code)
}

/// Classify an HTTP status code for circuit breaker error_type field.
fn classify_error_type(status_code: u16) -> &'static str {
    if (500..600).contains(&status_code) {
        "5xx"
    } else {
        "other"
    }
}
```

### Non-Streaming: Pre-Retry Circuit Filtering

```rust
// In chat_completions, after select_candidates succeeds:

// Filter candidates through circuit breaker (RTG-01)
let mut filtered = Vec::new();
let mut probe_provider: Option<String> = None;
for candidate in &candidates {
    match state.circuit_breakers.acquire_permit(&candidate.name).await {
        Ok(PermitType::Normal) => filtered.push(candidate),
        Ok(PermitType::Probe) => {
            probe_provider = Some(candidate.name.clone());
            filtered.insert(0, candidate); // probe goes first
        }
        Err(open_err) => {
            tracing::debug!(
                provider = %candidate.name,
                reason = %open_err.reason,
                "Skipping provider: circuit open"
            );
        }
    }
}

// Fail-fast if all circuits open (RTG-02)
if filtered.is_empty() {
    // return 503
}

// Create ProbeGuard if a probe candidate exists (MUST outlive timeout_at)
let probe_guard = probe_provider.as_ref().map(|name| {
    ProbeGuard::new(&state.circuit_breakers, name.clone())
});
```

### Non-Streaming: Post-Retry Outcome Recording

```rust
// After retry completes and attempt records are read:

// Record circuit breaker outcomes (RTG-03)
// Record failures for all failed attempts with 5xx status codes
for attempt in &recorded_attempts {
    if is_circuit_failure(attempt.status_code) {
        state.circuit_breakers.record_failure(
            &attempt.provider_name,
            classify_error_type(attempt.status_code),
            &format!("HTTP {}", attempt.status_code),
        );
    }
}

// Record success for the winning provider
match &retry_outcome.result {
    Ok(outcome) => {
        state.circuit_breakers.record_success(&outcome.provider_name);
    }
    Err(_) => {} // failures already recorded above
}

// Resolve probe guard
if let Some(guard) = probe_guard {
    match &retry_outcome.result {
        Ok(outcome) if outcome.provider_name == probe_provider.as_deref().unwrap_or("") => {
            guard.success();
        }
        _ => {
            guard.failure("5xx", "probe provider did not succeed");
        }
    }
}
```

### Streaming: Circuit Check and Outcome Recording

```rust
// Streaming path: check circuit for selected provider
// In execute_request or before send_to_provider:

let permit = state.circuit_breakers.acquire_permit(&provider.name).await;
match permit {
    Ok(PermitType::Normal) => {
        // Proceed with request
    }
    Ok(PermitType::Probe) => {
        // Create ProbeGuard, proceed with request, resolve after
    }
    Err(circuit_open) => {
        // Try next candidate or fail-fast 503
    }
}

// After send_to_provider returns Ok (2xx response):
// Record success immediately (per locked decision: 2xx = success regardless of stream)
state.circuit_breakers.record_success(&provider.name);
// If probe: probe_guard.success()
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Middleware-level circuit breaking | Handler-level integration | Phase 14 design decision | Gives handler full context (model, policy, candidates) for intelligent filtering |
| Record per-request | Record per-attempt | Phase 14 design | More accurate failure tracking -- each retry attempt counts independently |

**No deprecated patterns to avoid.** The entire circuit breaker infrastructure is fresh from Phase 13.

## Open Questions

1. **Streaming path candidate iteration**
   - What we know: The streaming path currently uses `execute_request` which calls `router.select()` (single provider). It does not iterate candidates.
   - What's unclear: Should the streaming path iterate multiple candidates if the first has an open circuit? Or just fail-fast?
   - Recommendation: The streaming path should attempt to find a candidate with a non-open circuit. Use `select_candidates()` instead of `select()`, then filter through `acquire_permit`. If all are open, 503. This gives streaming the same RTG-01/RTG-02 behavior as non-streaming.

2. **ProbeGuard ownership with Arc<CircuitBreakerRegistry>**
   - What we know: `ProbeGuard<'a>` borrows `&'a CircuitBreakerRegistry`. The registry is behind `Arc<CircuitBreakerRegistry>` in AppState. The handler holds a reference to state, so the registry lives for the handler's duration.
   - What's unclear: Whether `ProbeGuard`'s lifetime can span across the `timeout_at` boundary without issues.
   - Recommendation: Since `state` is passed by value (Clone) to the handler, `state.circuit_breakers` is an `Arc` that can be dereferenced. The ProbeGuard borrows from the dereferenced Arc, which lives as long as the handler function scope. The `timeout_at` does not move the ProbeGuard -- it only wraps the retry future. ProbeGuard stays in the outer scope. This should work cleanly.

3. **Concurrent acquire_permit calls in candidate filtering**
   - What we know: `acquire_permit` is async and may block for half-open wait. Filtering iterates candidates sequentially.
   - What's unclear: Whether sequential filtering introduces unacceptable latency.
   - Recommendation: Accept sequential filtering. With 1-10 providers (per REQUIREMENTS.md scope), at most one will be half-open at any time. The wait is bounded by the probe request duration (typically <5s). For a local proxy, this is acceptable.

## Sources

### Primary (HIGH confidence)
- `src/proxy/circuit_breaker.rs` -- Full circuit breaker API surface (acquire_permit, record_success, record_failure, ProbeGuard)
- `src/proxy/handlers.rs` -- Current handler structure for both streaming and non-streaming paths
- `src/proxy/retry.rs` -- Retry module with AttemptRecord tracking and is_retryable classification
- `src/router/selector.rs` -- Router select() and select_candidates() methods
- `src/proxy/server.rs` -- AppState with circuit_breakers field
- `src/error.rs` -- Error enum with IntoResponse for OpenAI-compatible errors
- `.planning/phases/13-circuit-breaker-state-machine/13-CONTEXT.md` -- Locked decisions on failure classification and streaming behavior
- `.planning/phases/13-circuit-breaker-state-machine/13-VERIFICATION.md` -- Verified Phase 13 API surface and test coverage

### Secondary (MEDIUM confidence)
- `.planning/REQUIREMENTS.md` -- RTG-01 through RTG-04 requirement definitions
- `.planning/ROADMAP.md` -- Phase 14 success criteria
- `.planning/STATE.md` -- Prior decisions (handler-level integration, pre-retry filtering)

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- No new dependencies, all integration uses existing crate APIs
- Architecture: HIGH -- Clear integration points identified from reading actual source code. Handler structure is well-understood.
- Pitfalls: HIGH -- Identified from actual code analysis (ProbeGuard lifetime, failure classification, attempt recording)

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable -- integration of already-built components, no external dependency changes)
