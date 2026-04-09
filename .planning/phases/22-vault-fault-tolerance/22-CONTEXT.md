# Phase 22: Vault Fault Tolerance - Context

**Gathered:** 2026-04-09
**Status:** Ready for planning

<domain>
## Phase Boundary

Verify and harden the existing pending settlement persistence and reconciliation loop so vault billing survives arbstr core crashes. Add eviction for stale settlements and comprehensive tests.

</domain>

<decisions>
## Implementation Decisions

### Test Strategy
- **D-01:** Two-level testing approach: (1) Direct DB insertion tests — insert PendingSettlement rows into SQLite, run reconciliation pass, verify they replay against mock vault (unit-level, deterministic). (2) Full cycle integration test — run a request with vault that fails mid-settle (timeout mock), verify pending row written, then run reconciliation and verify replay (end-to-end).
- **D-02:** Mock HTTP vault in Rust tests, same pattern as Phase 21 (from Phase 21 D-07).

### Reconciliation Behavior
- **D-03:** Evict pending settlements after 10 failed attempts. At 60s reconciliation intervals, that's ~10 minutes of retrying before giving up.
- **D-04:** Evicted settlements should be logged with `tracing::error` including reservation_id, type (settle/release), and attempt count. The row is deleted from pending_settlements table.
- **D-05:** No dead-letter table — eviction means deletion with error logging. Keeping it simple.

### Claude's Discretion
- Whether to add a configurable max_attempts field to VaultConfig or hardcode 10
- Exact test structure and naming conventions
- Whether reconciliation_pass return value should include eviction count

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Vault Fault Tolerance Implementation
- `src/proxy/vault.rs` — PendingSettlement struct (line 331), insert/fetch/delete/replay functions (lines 345-445), reconciliation_loop (line 453)
- `src/proxy/server.rs` — Reconciliation task spawn on startup (lines 236-249)
- `src/proxy/handlers.rs` — spawn_vault_settle/spawn_vault_release with pending settlement fallback

### Prior Phase Context
- `.planning/phases/21-vault-billing-wiring/21-CONTEXT.md` — D-07 (mock HTTP vault in tests), D-01 (auth token handling)

### Research
- `.planning/research/PITFALLS.md` — Pitfall 2 (crash between response and vault call), Pitfall 9 (5s vault timeout creating duplicate reserves)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `PendingSettlement` struct with type/reservation_id/amount_msats/metadata fields — fully implemented
- `insert_pending_settlement`, `fetch_pending`, `delete_pending`, `increment_attempts` — all working
- `replay_pending` — replays a single settlement against vault with settle/release dispatch
- `reconciliation_loop` — tokio::select! with 60s interval and cancellation token
- Mock vault test infrastructure from Phase 21 (`tests/vault_billing.rs`, `tests/common/mod.rs`)

### Established Patterns
- Reconciliation uses `tokio::select!` with `watch::Receiver` for graceful shutdown
- Pending settlements use direct sqlx (bypass bounded DbWriter) for guaranteed persistence
- Backpressure flag (`AtomicBool`) set when pending count exceeds threshold

### Integration Points
- `reconciliation_pass` (vault.rs) — needs max_attempts check added to evict stale entries
- `increment_attempts` — currently unconditional, needs to check threshold and evict
- Test infrastructure in `tests/common/mod.rs` — extend with pending settlement helpers

</code_context>

<specifics>
## Specific Ideas

- Eviction after 10 attempts with tracing::error, simple delete (no dead-letter table)
- Both direct-insertion unit tests and full-cycle integration test for comprehensive coverage

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 22-vault-fault-tolerance*
*Context gathered: 2026-04-09*
