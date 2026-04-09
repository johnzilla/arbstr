# Phase 21: Vault Billing Wiring - Context

**Gathered:** 2026-04-09
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire the existing VaultClient (src/proxy/vault.rs) into the handler hot path so every inference request goes through reserve/settle/release billing against arbstr vault. Preserve free proxy mode when vault is not configured.

</domain>

<decisions>
## Implementation Decisions

### Auth Token Handling
- **D-01:** `Authorization: Bearer` header serves both purposes depending on config. When vault is configured, the bearer token is forwarded to vault as the agent_token for billing. When vault is absent but auth_token is set, bearer token is checked against server-level auth_token. When neither is configured, no auth required.
- **D-02:** ~~SUPERSEDED~~ Originally "both layers active simultaneously" — superseded by D-01 after resolving the header conflict. When vault is configured, vault handles auth; server-level auth_token middleware is skipped. This matches existing code at server.rs lines 103-115.

### Reserve Pricing
- **D-03:** Always reserve at frontier-tier rates regardless of the complexity scorer's tier result. This handles tier escalation safely — if a local-tier request escalates to frontier on circuit break, the reservation already covers the higher cost. Overage is refunded on settle.
- **D-04:** `estimate_reserve_msats` must be updated to find the most expensive (frontier-tier) provider rates for the requested model, not the cheapest candidate's rates.

### Error Responses
- **D-05:** Vault errors (402 insufficient balance, 403 policy denied, 429 rate limited) are wrapped in OpenAI-compatible JSON error format (`{"error": {"message": ..., "type": ..., "code": ...}}`). This is consistent with existing error handling in error.rs and ensures clients get uniform error format.
- **D-06:** Vault HTTP status codes are preserved (402→402, 403→403, 429→429) but the response body uses arbstr's OpenAI-compatible error structure, not vault's native format.

### Testing
- **D-07:** Vault billing tested via mock HTTP vault in Rust integration tests. Spin up a lightweight HTTP server (e.g., using axum in tests) that mimics vault's /internal/reserve, /internal/settle, /internal/release responses. No TypeScript/Node dependency in test suite.
- **D-08:** Real vault integration (arbstr-vault in simulated mode) is for manual E2E testing via Docker Compose, not automated CI.

### Free Proxy Mode
- **D-09:** When vault config is absent, all vault-related code paths are no-ops. Behavior must be identical to pre-v2.0. This is already partially implemented — verify no regressions.

### Claude's Discretion
- Implementation details of the mock HTTP vault server in tests
- Exact OpenAI error type/code strings for vault errors
- Whether to add new error variants to error.rs or reuse existing ones

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Vault Integration (arbstr core)
- `src/proxy/vault.rs` — Full VaultClient implementation (reserve/settle/release, retry, pending persistence, reconciliation loop)
- `src/proxy/handlers.rs` — Handler hot path with existing vault integration (backpressure check, agent token extraction, reserve call, settle/release spawning)
- `src/proxy/server.rs` — Server setup including auth_token middleware (lines 47-104)
- `src/config.rs` — VaultConfig struct and ServerConfig.auth_token field

### Vault Integration (arbstr vault)
- Vault /internal/reserve, /internal/settle, /internal/release endpoints (github.com/johnzilla/arbstr-vault src/routes/internal/reserve.routes.ts)

### Research
- `.planning/research/PITFALLS.md` — Critical pitfalls for vault billing (reserve under-estimation, crash recovery, timeout-induced duplicate reserves)
- `.planning/research/ARCHITECTURE.md` — Integration architecture and data flow

### PRD
- `~/Downloads/arbstr-prd.md` — Product requirements document with payment flow and two-tier auth design

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `VaultClient` (vault.rs): Complete reserve/settle/release with retry (3 attempts, exponential backoff), backpressure detection, pending settlement persistence
- `spawn_vault_settle` / `spawn_vault_release` (handlers.rs): Fire-and-forget background tasks with DB fallback on failure
- `estimate_reserve_msats` (vault.rs): Cost estimation function — needs modification for frontier-tier pricing
- `ArbstrError` (error.rs): OpenAI-compatible error responses — extend with vault error variants

### Established Patterns
- Vault integration is gated by `if let Some(vault) = &state.vault` — no-op when absent
- Fire-and-forget settle/release via tokio::spawn — never blocks response path
- Pending settlements use direct sqlx (bypass bounded DbWriter) for guaranteed persistence

### Integration Points
- `handlers.rs` line 557: Main vault integration point in request handler
- `server.rs` line 92-104: Auth middleware — must coexist with vault auth per D-01/D-02
- `config.rs` VaultConfig: May need new field for frontier rate lookup

</code_context>

<specifics>
## Specific Ideas

- Auth uses standard `Authorization: Bearer` header for both server auth and vault auth — resolved by config state, no custom headers
- Reserve always at frontier rates — simple and safe over complex and precise
- OpenAI-compatible error format for all vault errors — clients get uniform error handling

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 21-vault-billing-wiring*
*Context gathered: 2026-04-09*
