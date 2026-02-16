# Roadmap: arbstr

## Milestones

- SHIPPED **v1 Reliability and Observability** -- Phases 1-4 (shipped 2026-02-04)
- SHIPPED **v1.1 Secrets Hardening** -- Phases 5-7 (shipped 2026-02-15)
- SHIPPED **v1.2 Streaming Observability** -- Phases 8-10 (shipped 2026-02-16)
- IN PROGRESS **v1.3 Cost Querying API** -- Phases 11-12

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

### v1.3 Cost Querying API

- [x] **Phase 11: Aggregate Stats and Filtering** - Read-only stats endpoints with time range scoping, presets, and model/provider filtering (completed 2026-02-16)
- [ ] **Phase 12: Request Log Listing** - Paginated request log browsing with filtering and sorting

## Phase Details

### Phase 11: Aggregate Stats and Filtering
**Goal**: Users can query aggregate cost and performance data from arbstr's SQLite logs, scoped by time range and filtered by model or provider
**Depends on**: Phase 10 (existing SQLite logging infrastructure)
**Requirements**: STAT-01, STAT-02, FILT-01, FILT-02, FILT-03
**Success Criteria** (what must be TRUE):
  1. User can GET an aggregate summary (total requests, total cost sats, total input/output tokens, avg latency, success rate, error count, streaming count) and receive a JSON response with all fields populated
  2. User can GET per-model stats and see the same aggregate fields broken down by model name
  3. User can pass `since` and `until` ISO 8601 query parameters to scope any stats response to an arbitrary time window
  4. User can pass a `range` query parameter (last_1h, last_24h, last_7d, last_30d) as a shortcut instead of explicit timestamps
  5. User can pass `model` or `provider` query parameters to narrow stats to a specific model or provider
**Plans:** 2/2 plans complete

Plans:
- [ ] 11-01-PLAN.md -- Storage layer, handler, and route wiring for /v1/stats endpoint
- [ ] 11-02-PLAN.md -- Integration tests for /v1/stats endpoint

### Phase 12: Request Log Listing
**Goal**: Users can browse and investigate individual request records with flexible filtering and sorting
**Depends on**: Phase 11 (read-only pool, time range helpers, route namespace)
**Requirements**: LOG-01, LOG-02, LOG-03
**Success Criteria** (what must be TRUE):
  1. User can GET a paginated list of individual request records with page number, per-page size, and total count in the response
  2. User can filter request logs by model, provider, success status, or streaming flag via query parameters
  3. User can sort request logs by timestamp, cost, or latency in ascending or descending order via query parameters
**Plans**: TBD

Plans:
- [ ] 12-01: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 11 -> 12

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Foundation | v1 | 2/2 | Complete | 2026-02-02 |
| 2. Request Logging | v1 | 4/4 | Complete | 2026-02-04 |
| 3. Response Metadata | v1 | 1/1 | Complete | 2026-02-04 |
| 4. Retry and Fallback | v1 | 3/3 | Complete | 2026-02-04 |
| 5. Secret Type Foundation | v1.1 | 1/1 | Complete | 2026-02-15 |
| 6. Environment Variable Expansion | v1.1 | 2/2 | Complete | 2026-02-15 |
| 7. Output Surface Hardening | v1.1 | 1/1 | Complete | 2026-02-15 |
| 8. Stream Request Foundation | v1.2 | 1/1 | Complete | 2026-02-16 |
| 9. SSE Stream Interception | v1.2 | 2/2 | Complete | 2026-02-16 |
| 10. Streaming Observability Integration | v1.2 | 1/1 | Complete | 2026-02-16 |
| 11. Aggregate Stats and Filtering | v1.3 | Complete    | 2026-02-16 | - |
| 12. Request Log Listing | v1.3 | 0/? | Not started | - |
