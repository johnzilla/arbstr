# Roadmap: arbstr

## Milestones

- SHIPPED **v1 Reliability and Observability** -- Phases 1-4 (shipped 2026-02-04)
- SHIPPED **v1.1 Secrets Hardening** -- Phases 5-7 (shipped 2026-02-15)
- SHIPPED **v1.2 Streaming Observability** -- Phases 8-10 (shipped 2026-02-16)
- SHIPPED **v1.3 Cost Querying API** -- Phases 11-12 (shipped 2026-02-16)
- IN PROGRESS **v1.4 Circuit Breaker** -- Phases 13-15

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

### v1.4 Circuit Breaker

- [x] **Phase 13: Circuit Breaker State Machine** - Per-provider 3-state circuit breaker with consecutive failure tracking and half-open recovery (completed 2026-02-16)
- [x] **Phase 14: Routing Integration** - Handler-level circuit filtering, fail-fast 503, and outcome recording for both streaming and non-streaming paths (completed 2026-02-16)
- [ ] **Phase 15: Enhanced Health Endpoint** - Per-provider circuit state and degraded/unhealthy status reporting via /health

## Phase Details

### Phase 13: Circuit Breaker State Machine
**Goal**: Each provider has a correct, independently testable circuit breaker that tracks failures and recovers automatically
**Depends on**: Nothing (foundation for v1.4)
**Requirements**: CB-01, CB-02, CB-03, CB-04, CB-05, CB-06
**Success Criteria** (what must be TRUE):
  1. Each provider has its own circuit breaker with Closed, Open, and Half-Open states
  2. Circuit opens after 3 consecutive 5xx/timeout failures and ignores 4xx responses
  3. Successful request resets the failure counter to zero (non-consecutive failures never trip)
  4. After 30 seconds in Open state, circuit transitions to Half-Open and allows exactly one probe request
  5. Probe success closes the circuit; probe failure reopens it with a fresh 30s timer
**Plans**: 2 plans

Plans:
- [ ] 13-01-PLAN.md -- Core circuit breaker state machine (TDD: types, transitions, unit tests)
- [ ] 13-02-PLAN.md -- Registry, queue-and-wait, ProbeGuard, AppState wiring

### Phase 14: Routing Integration
**Goal**: Router uses circuit state to skip unhealthy providers, with fail-fast when no alternatives exist
**Depends on**: Phase 13
**Requirements**: RTG-01, RTG-02, RTG-03, RTG-04
**Success Criteria** (what must be TRUE):
  1. Non-streaming requests skip providers with open circuits and route to the next cheapest available provider
  2. When all providers for a requested model have open circuits, the proxy returns 503 immediately without attempting any requests
  3. After a non-streaming request completes (including retries), the circuit breaker records the outcome for each attempted provider
  4. After a streaming response completes in the background task, the circuit breaker records whether the stream succeeded or failed
**Plans**: 2 plans

Plans:
- [ ] 14-01-PLAN.md -- Error::CircuitOpen variant, non-streaming circuit filtering, and outcome recording
- [ ] 14-02-PLAN.md -- Streaming circuit filtering, outcome recording, and integration tests

### Phase 15: Enhanced Health Endpoint
**Goal**: Operators can see per-provider circuit health and overall system status at a glance
**Depends on**: Phase 13
**Requirements**: HLT-01, HLT-02
**Success Criteria** (what must be TRUE):
  1. GET /health returns a JSON response containing each provider's circuit state (closed, open, or half_open) and its current failure count
  2. Top-level status field reads "ok" when all circuits are closed, "degraded" when some are open, and "unhealthy" when all are open
**Plans**: TBD

Plans:
- [ ] 15-01: TBD

## Progress

**Execution Order:** 13 -> 14 -> 15 (Phase 15 depends on 13 only, could run after 13 in parallel with 14)

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 13. Circuit Breaker State Machine | v1.4 | Complete    | 2026-02-16 | - |
| 14. Routing Integration | v1.4 | Complete    | 2026-02-16 | - |
| 15. Enhanced Health Endpoint | v1.4 | 0/1 | Not started | - |
