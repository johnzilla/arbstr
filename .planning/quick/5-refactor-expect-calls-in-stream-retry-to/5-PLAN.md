---
phase: quick-5
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - src/proxy/circuit_breaker.rs
autonomous: true
requirements: [QUICK-5]

must_haves:
  truths:
    - "A poisoned mutex in circuit_breaker.rs does not crash the server"
    - "All production .unwrap() calls on Mutex::lock() use poisoned mutex recovery"
    - "Existing circuit breaker tests pass unchanged"
  artifacts:
    - path: "src/proxy/circuit_breaker.rs"
      provides: "Poisoned mutex recovery on all 9 lock() call sites"
      contains: "unwrap_or_else"
  key_links:
    - from: "src/proxy/circuit_breaker.rs"
      to: "src/proxy/handlers.rs"
      via: "CircuitBreakerRegistry methods called from request handlers"
      pattern: "registry\\.(acquire_permit|record_success|record_failure|record_probe)"
---

<objective>
Replace all 9 `.unwrap()` calls on `Mutex::lock()` in circuit_breaker.rs production code with
`unwrap_or_else(|e| e.into_inner())` for poisoned mutex recovery.

Purpose: Prevent server panics if a mutex becomes poisoned (e.g., due to a panic in another
thread holding the lock). This is the same pattern already used in stream.rs and retry.rs.

Note: The original task mentions stream.rs and retry.rs, but those files already have proper
error handling in production code (stream.rs uses `unwrap_or_else` everywhere, retry.rs has
one provably-unreachable `.expect()` retained per quick-3 decision). The remaining production
`.unwrap()` calls on mutexes are all in circuit_breaker.rs.

Output: Updated src/proxy/circuit_breaker.rs with consistent poisoned mutex recovery.
</objective>

<execution_context>
@/home/john/.claude/get-shit-done/workflows/execute-plan.md
@/home/john/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/proxy/circuit_breaker.rs
</context>

<tasks>

<task type="auto">
  <name>Task 1: Replace mutex .unwrap() with poisoned mutex recovery in circuit_breaker.rs</name>
  <files>src/proxy/circuit_breaker.rs</files>
  <action>
Replace all 9 `.unwrap()` calls on `Mutex::lock()` in production code (NOT in `#[cfg(test)]`)
with `.unwrap_or_else(|e| e.into_inner())`. This recovers the inner data from a poisoned mutex
instead of panicking, matching the pattern already established in stream.rs (line 311) and
retry.rs (line 154).

The 9 call sites are in these methods of `CircuitBreakerRegistry`:
1. `acquire_permit` (line 344): `cb.inner.lock().unwrap()`
2. `record_success` (line 405): `entry.value().inner.lock().unwrap()`
3. `record_failure` (line 415): `entry.value().inner.lock().unwrap()`
4. `record_probe_success` (line 429): `cb.inner.lock().unwrap()`
5. `record_probe_failure` (line 443): `cb.inner.lock().unwrap()`
6. `all_states` (line 456): `entry.value().inner.lock().unwrap()`
7. `state` (line 470): `entry.value().inner.lock().unwrap().state`
8. `failure_count` (line 477): `entry.value().inner.lock().unwrap().failure_count`
9. `trip_count` (line 484): `entry.value().inner.lock().unwrap().trip_count`

For each, change `.unwrap()` to `.unwrap_or_else(|e| e.into_inner())`.

Do NOT modify any code inside `#[cfg(test)] mod tests { ... }` -- test code can use `.unwrap()`
since panics in tests are expected behavior.
  </action>
  <verify>
    <automated>cd /home/john/vault/projects/github.com/arbstr && cargo test --lib proxy::circuit_breaker && cargo clippy -- -D warnings</automated>
  </verify>
  <done>All 9 mutex lock sites in circuit_breaker.rs production code use unwrap_or_else for poisoned mutex recovery. All existing tests pass. No clippy warnings.</done>
</task>

</tasks>

<verification>
- `grep -c 'lock().unwrap()' src/proxy/circuit_breaker.rs` in production code (before `#[cfg(test)]`) returns 0
- `grep -c 'unwrap_or_else' src/proxy/circuit_breaker.rs` in production code shows 9 occurrences
- `cargo test` passes all tests
- `cargo clippy -- -D warnings` has no warnings
</verification>

<success_criteria>
- Zero `.unwrap()` calls on `Mutex::lock()` in production code of circuit_breaker.rs
- All 9 call sites use `unwrap_or_else(|e| e.into_inner())` pattern
- All existing tests pass without modification
- Consistent with the poisoned mutex recovery pattern in stream.rs and retry.rs
</success_criteria>

<output>
After completion, create `.planning/quick/5-refactor-expect-calls-in-stream-retry-to/5-SUMMARY.md`
</output>
