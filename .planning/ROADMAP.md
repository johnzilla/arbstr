# Roadmap: arbstr

## Milestones

- SHIPPED **v1 Reliability and Observability** -- Phases 1-4 (shipped 2026-02-04)
- SHIPPED **v1.1 Secrets Hardening** -- Phases 5-7 (shipped 2026-02-15)
- SHIPPED **v1.2 Streaming Observability** -- Phases 8-10 (shipped 2026-02-16)
- SHIPPED **v1.3 Cost Querying API** -- Phases 11-12 (shipped 2026-02-16)
- SHIPPED **v1.4 Circuit Breaker** -- Phases 13-15 (shipped 2026-02-16)
- SHIPPED **v1.7 Intelligent Complexity Routing** -- Phases 16-20 (shipped 2026-04-09)
- IN PROGRESS **v2.0 Inference Marketplace Foundation** -- Phases 21-25

## Phases

<details>
<summary>SHIPPED v1 Reliability and Observability (Phases 1-4) -- SHIPPED 2026-02-04</summary>

- [x] Phase 1: Foundation (2/2 plans) -- completed 2026-02-02
- [x] Phase 2: Request Logging (4/4 plans) -- completed 2026-02-04
- [x] Phase 3: Response Metadata (1/1 plan) -- completed 2026-02-04
- [x] Phase 4: Retry and Fallback (3/3 plans) -- completed 2026-02-04

See: .planning/milestones/v1-ROADMAP.md for full details.

</details>

<details>
<summary>SHIPPED v1.1 Secrets Hardening (Phases 5-7) -- SHIPPED 2026-02-15</summary>

- [x] Phase 5: Secret Type Foundation (1/1 plan) -- completed 2026-02-15
- [x] Phase 6: Environment Variable Expansion (2/2 plans) -- completed 2026-02-15
- [x] Phase 7: Output Surface Hardening (1/1 plan) -- completed 2026-02-15

See: .planning/milestones/v1.1-ROADMAP.md for full details.

</details>

<details>
<summary>SHIPPED v1.2 Streaming Observability (Phases 8-10) -- SHIPPED 2026-02-16</summary>

- [x] Phase 8: Stream Request Foundation (1/1 plan) -- completed 2026-02-16
- [x] Phase 9: SSE Stream Interception (2/2 plans) -- completed 2026-02-16
- [x] Phase 10: Streaming Observability Integration (1/1 plan) -- completed 2026-02-16

See: .planning/milestones/v1.2-ROADMAP.md for full details.

</details>

<details>
<summary>SHIPPED v1.3 Cost Querying API (Phases 11-12) -- SHIPPED 2026-02-16</summary>

- [x] Phase 11: Aggregate Stats and Filtering (2/2 plans) -- completed 2026-02-16
- [x] Phase 12: Request Log Listing (2/2 plans) -- completed 2026-02-16

See: .planning/milestones/v1.3-ROADMAP.md for full details.

</details>

<details>
<summary>SHIPPED v1.4 Circuit Breaker (Phases 13-15) -- SHIPPED 2026-02-16</summary>

- [x] Phase 13: Circuit Breaker State Machine (2/2 plans) -- completed 2026-02-16
- [x] Phase 14: Routing Integration (2/2 plans) -- completed 2026-02-16
- [x] Phase 15: Enhanced Health Endpoint (1/1 plan) -- completed 2026-02-16

See: .planning/milestones/v1.4-ROADMAP.md for full details.

</details>

<details>
<summary>SHIPPED v1.7 Intelligent Complexity Routing (Phases 16-20) -- SHIPPED 2026-04-09</summary>

- [x] Phase 16: Provider Tier Foundation (2/2 plans) -- completed 2026-04-08
- [x] Phase 17: Complexity Scorer (1/1 plan) -- completed 2026-04-08
- [x] Phase 18: Tier-Aware Routing (1/1 plan) -- completed 2026-04-08
- [x] Phase 19: Handler Integration and Escalation (1/1 plan) -- completed 2026-04-09
- [x] Phase 20: Routing Observability (2/2 plans) -- completed 2026-04-09

See: .planning/milestones/v1.7-ROADMAP.md for full details.

</details>

### v2.0 Inference Marketplace Foundation (In Progress)

**Milestone Goal:** Wire arbstr core to arbstr vault for live billing, add mesh-llm as a provider type, ship arbstr-node deployment, and launch arbstr.com.

- [x] **Phase 21: Vault Billing Wiring** - End-to-end reserve/settle/release flow with agent token auth (completed 2026-04-09)
- [ ] **Phase 22: Vault Fault Tolerance** - Pending settlement persistence and crash recovery
- [ ] **Phase 23: Docker Deployment** - Multi-stage Dockerfile and compose full-stack deployment
- [ ] **Phase 24: mesh-llm Provider** - mesh-llm as first-class provider with model auto-discovery
- [ ] **Phase 25: Landing Page** - arbstr.com with marketplace positioning and getting started guide

## Phase Details

### Phase 21: Vault Billing Wiring
**Goal**: Every inference request is billed through arbstr vault -- reserve before routing, settle on success, release on failure
**Depends on**: Phase 20 (existing vault.rs client code)
**Requirements**: BILL-01, BILL-02, BILL-03, BILL-04, BILL-05, BILL-06, BILL-08
**Success Criteria** (what must be TRUE):
  1. Sending a chat completion with a valid agent token reserves funds, routes inference, and settles actual cost to vault
  2. A failed inference request releases the reservation back to the buyer's account (no funds lost)
  3. Reserve amount uses frontier-tier pricing regardless of scored tier, so tier escalation never under-reserves
  4. When vault config is absent, proxy operates identically to pre-v2.0 behavior (free proxy mode)
  5. Agent bearer token from Authorization header is forwarded to vault and replaces server-level auth_token for proxy endpoints
**Plans:** 2/2 plans complete

Plans:
- [x] 21-01-PLAN.md -- Fix reserve pricing to use frontier-tier rates and add Router.frontier_rates method
- [x] 21-02-PLAN.md -- Mock vault integration tests verifying end-to-end billing flow

### Phase 22: Vault Fault Tolerance
**Goal**: Vault billing survives crashes -- unsettled reservations are persisted and replayed on restart
**Depends on**: Phase 21
**Requirements**: BILL-07
**Success Criteria** (what must be TRUE):
  1. If arbstr core crashes between reserve and settle, pending settlements are recovered from SQLite on restart
  2. Background reconciliation replays pending settle/release operations against vault after restart
**Plans:** 1 plan

Plans:
- [ ] 22-01-PLAN.md -- Add eviction logic for stale settlements and comprehensive fault tolerance tests

### Phase 23: Docker Deployment
**Goal**: arbstr-node runs as a complete stack from a single docker compose up command
**Depends on**: Phase 21 (vault must be wired for compose to be meaningful)
**Requirements**: DEPLOY-01, DEPLOY-02, DEPLOY-03
**Success Criteria** (what must be TRUE):
  1. Multi-stage Dockerfile produces a slim arbstr core image (builder stage + runtime stage)
  2. docker compose up from empty volumes starts lnd, mint, vault, and core in correct dependency order with health checks
  3. A chat completion request through the composed stack returns a successful response with billing headers
**Plans:** 1 plan

Plans:
- [ ] 22-01-PLAN.md -- Add eviction logic for stale settlements and comprehensive fault tolerance tests

### Phase 24: mesh-llm Provider
**Goal**: mesh-llm nodes on localhost are usable as zero-cost local-tier providers with automatic model discovery
**Depends on**: Phase 23 (Docker networking needed for compose integration)
**Requirements**: MESH-01, MESH-02, MESH-03
**Success Criteria** (what must be TRUE):
  1. mesh-llm endpoint at localhost:9337 is configurable as a provider with tier=local and zero-cost rates
  2. On startup, arbstr polls mesh-llm /v1/models and auto-populates the provider's available model list
  3. Docker Compose core service can reach mesh-llm running on the host via extra_hosts configuration
**Plans:** 1 plan

Plans:
- [ ] 22-01-PLAN.md -- Add eviction logic for stale settlements and comprehensive fault tolerance tests

### Phase 25: Landing Page
**Goal**: arbstr.com communicates the marketplace vision and gets developers started
**Depends on**: Nothing (fully independent, but sequenced last so shipped features inform copy)
**Requirements**: DEPLOY-04
**Success Criteria** (what must be TRUE):
  1. arbstr.com loads with marketplace positioning (NiceHash for AI inference) and anti-token manifesto
  2. Getting started guide shows how to run arbstr-node with docker compose up
  3. Page links to GitHub repo and explains the Bitcoin-native settlement model
**Plans:** 1 plan

Plans:
- [ ] 22-01-PLAN.md -- Add eviction logic for stale settlements and comprehensive fault tolerance tests
**UI hint**: yes

## Progress

**Execution Order:**
Phases execute in numeric order: 21 -> 22 -> 23 -> 24 -> 25

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 21. Vault Billing Wiring | v2.0 | 2/2 | Complete    | 2026-04-09 |
| 22. Vault Fault Tolerance | v2.0 | 0/TBD | Not started | - |
| 23. Docker Deployment | v2.0 | 0/TBD | Not started | - |
| 24. mesh-llm Provider | v2.0 | 0/TBD | Not started | - |
| 25. Landing Page | v2.0 | 0/TBD | Not started | - |
