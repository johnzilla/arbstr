# Requirements: arbstr

**Defined:** 2026-04-09
**Core Value:** Route inference to the cheapest qualified provider and settle in bitcoin — NiceHash for AI inference.

## v2.0 Requirements

Requirements for Inference Marketplace Foundation milestone. Each maps to roadmap phases.

### Vault Billing

- [ ] **BILL-01**: arbstr core extracts agent bearer token from Authorization header and forwards to vault reserve
- [ ] **BILL-02**: Each inference request reserves funds from buyer's vault account before routing
- [ ] **BILL-03**: Successful inference settles actual cost to vault with token/provider/latency metadata
- [ ] **BILL-04**: Failed inference releases reservation back to buyer's vault account
- [ ] **BILL-05**: Reserve amount uses worst-case (frontier-tier) pricing to handle tier escalation safely
- [ ] **BILL-06**: When vault is configured, vault agent token replaces server-level auth_token for proxy endpoints
- [ ] **BILL-07**: Pending settlements persist to SQLite and replay via background reconciliation on restart
- [ ] **BILL-08**: Vault billing is gracefully skipped when vault config is absent (free proxy mode preserved)

### mesh-llm

- [ ] **MESH-01**: mesh-llm endpoint configurable as a standard provider with tier=local and zero-cost rates
- [ ] **MESH-02**: Core polls mesh-llm /v1/models to auto-populate available models on startup
- [ ] **MESH-03**: Docker Compose core service can reach mesh-llm on host via extra_hosts configuration

### Deployment

- [ ] **DEPLOY-01**: Multi-stage Dockerfile for arbstr core (Rust builder + slim runtime)
- [ ] **DEPLOY-02**: Docker Compose health check chain verified (lnd → mint → vault → core startup order)
- [ ] **DEPLOY-03**: Full stack starts cleanly from empty volumes with `docker compose up`
- [ ] **DEPLOY-04**: arbstr.com landing page with marketplace positioning, anti-token manifesto, and getting started guide

## Future Requirements

### Discovery

- **DISC-01**: Providers publish model profiles to Pubky Homeservers
- **DISC-02**: arbstr core discovers providers via Pubky DHT
- **DISC-03**: Provider availability mirrored to Nostr relays as bridge

### Marketplace

- **MKT-01**: Seller account type in vault with Lightning withdrawal support
- **MKT-02**: Batch settlement (accumulate seller credits → Lightning payout at threshold)
- **MKT-03**: Provider reputation via Pubky semantic tags
- **MKT-04**: Cross-node federation (discover providers on other arbstr nodes)

### Auth

- **AUTH-01**: L402 anonymous access tier (HTTP 402 → Lightning invoice → macaroon)

## Out of Scope

| Feature | Reason |
|---------|--------|
| Web dashboard UI | Query endpoints sufficient, CLI or curl for now |
| ML-based policy classification | Keyword heuristics sufficient |
| Cross-model fallback | Silently substituting cheaper model changes quality |
| Invented tokens / governance / staking | Bitcoin is the only money — core brand identity |
| Multi-mint Cashu support | Single self-hosted mint sufficient for v2.0 |
| Buyer Cashu deposit flow | Manual balance seeding sufficient for v2.0 testing |
| Pubky/Nostr discovery | Deferred to discovery milestone |
| L402 anonymous access | Deferred to auth milestone |

## Traceability

Which phases cover which requirements. Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|
| BILL-01 | Phase 21 | Pending |
| BILL-02 | Phase 21 | Pending |
| BILL-03 | Phase 21 | Pending |
| BILL-04 | Phase 21 | Pending |
| BILL-05 | Phase 21 | Pending |
| BILL-06 | Phase 21 | Pending |
| BILL-07 | Phase 22 | Pending |
| BILL-08 | Phase 21 | Pending |
| MESH-01 | Phase 24 | Pending |
| MESH-02 | Phase 24 | Pending |
| MESH-03 | Phase 24 | Pending |
| DEPLOY-01 | Phase 23 | Pending |
| DEPLOY-02 | Phase 23 | Pending |
| DEPLOY-03 | Phase 23 | Pending |
| DEPLOY-04 | Phase 25 | Pending |

**Coverage:**
- v2.0 requirements: 15 total
- Mapped to phases: 15
- Unmapped: 0

---
*Requirements defined: 2026-04-09*
*Last updated: 2026-04-09 after roadmap creation*
