# Roadmap: arbstr

## Milestones

- SHIPPED **v1 Reliability and Observability** -- Phases 1-4 (shipped 2026-02-04)
- SHIPPED **v1.1 Secrets Hardening** -- Phases 5-7 (shipped 2026-02-15)
- SHIPPED **v1.2 Streaming Observability** -- Phases 8-10 (shipped 2026-02-16)
- SHIPPED **v1.3 Cost Querying API** -- Phases 11-12 (shipped 2026-02-16)
- SHIPPED **v1.4 Circuit Breaker** -- Phases 13-15 (shipped 2026-02-16)
- IN PROGRESS **v1.7 Intelligent Complexity Routing** -- Phases 16-20

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

### v1.7 Intelligent Complexity Routing (In Progress)

**Milestone Goal:** Route simple requests to cheap/local providers and escalate complex requests to frontier models automatically -- the user never picks a model.

- [x] **Phase 16: Provider Tier Foundation** - Tier enum and provider config field with backward compatibility (completed 2026-04-08)
- [x] **Phase 17: Complexity Scorer** - Heuristic scoring engine with configurable signal weights (completed 2026-04-08)
- [x] **Phase 18: Tier-Aware Routing** - Router filters by tier based on complexity score and thresholds (completed 2026-04-08)
- [x] **Phase 19: Handler Integration and Escalation** - End-to-end pipeline with header override and circuit-break escalation (completed 2026-04-09)
- [x] **Phase 20: Routing Observability** - Response headers, SSE metadata, DB columns, and stats breakdown (completed 2026-04-09)

## Phase Details

### Phase 16: Provider Tier Foundation
**Goal**: Providers can be classified into tiers (local/standard/frontier) and existing configs parse unchanged
**Depends on**: Phase 15
**Requirements**: TIER-01, TIER-02, TIER-03
**Success Criteria** (what must be TRUE):
  1. A provider config with `tier = "local"` parses and the tier value is accessible in routing
  2. A provider config without a `tier` field parses successfully and defaults to `standard`
  3. All existing config.toml files and tests pass without modification (backward compatible)
**Plans:** 2/2 plans complete
Plans:
- [x] 16-01-PLAN.md -- Tier enum, RoutingConfig, ComplexityWeightsConfig, tier field on provider configs
- [x] 16-02-PLAN.md -- Propagate tier through SelectedProvider, /providers, and /health endpoints

### Phase 17: Complexity Scorer
**Goal**: Every request receives a complexity score (0.0-1.0) from a heuristic scorer that analyzes the full conversation
**Depends on**: Phase 16
**Requirements**: SCORE-01, SCORE-02, SCORE-04, SCORE-05
**Success Criteria** (what must be TRUE):
  1. A simple "hello" message scores below the low threshold; a multi-turn conversation with code blocks and reasoning keywords scores above the high threshold
  2. Signal weights are configurable in `[routing.complexity_weights]` and the scorer uses them
  3. The scorer operates on the full messages array (not just the last message) and conversation depth affects the score
  4. An ambiguous or unclassifiable prompt defaults to a high score (routes to frontier)
**Plans:** 1/1 plans complete
Plans:
- [x] 17-01-PLAN.md -- Complexity scorer with 5 weighted signals, unit tests, re-export from router

### Phase 18: Tier-Aware Routing
**Goal**: The router selects providers from the appropriate tier based on complexity score and configurable thresholds
**Depends on**: Phase 17
**Requirements**: ROUTE-01, ROUTE-02, ROUTE-03
**Success Criteria** (what must be TRUE):
  1. A low-complexity request is routed only to local-tier providers (when available)
  2. A mid-complexity request is routed to local or standard-tier providers
  3. A high-complexity request can be routed to any tier (including frontier)
  4. Thresholds between tiers are configurable via `complexity_threshold_low` and `complexity_threshold_high` in `[routing]`
**Plans:** 1/1 plans complete
Plans:
- [x] 18-01-PLAN.md -- score_to_max_tier mapping, max_tier parameter on select_candidates/select, tier filter predicate

### Phase 19: Handler Integration and Escalation
**Goal**: The scoring-routing pipeline works end-to-end with header override and automatic tier escalation when providers are unhealthy
**Depends on**: Phase 18
**Requirements**: SCORE-03, ROUTE-04, ROUTE-05
**Success Criteria** (what must be TRUE):
  1. Sending `X-Arbstr-Complexity: high` header bypasses the scorer and routes to frontier-capable providers
  2. Sending `X-Arbstr-Complexity: low` header bypasses the scorer and routes to local-tier providers
  3. When all local-tier providers have open circuits, a low-complexity request automatically escalates to standard tier (and then frontier if needed)
  4. Escalation is one-way per request -- a request that escalated from local to standard never de-escalates back to local
**Plans:** 1/1 plans complete
Plans:
- [x] 19-01-PLAN.md -- Wire scoring, header override, and escalation into resolve_candidates

### Phase 20: Routing Observability
**Goal**: Complexity scores and tier decisions are visible in response headers, SSE metadata, request logs, and stats
**Depends on**: Phase 19
**Requirements**: OBS-01, OBS-02, OBS-03, OBS-04, OBS-05
**Success Criteria** (what must be TRUE):
  1. Non-streaming responses include `x-arbstr-complexity-score` and `x-arbstr-tier` headers
  2. Streaming responses include complexity score and tier in the trailing SSE metadata event
  3. The requests table has `complexity_score` and `tier` columns populated for every request
  4. `GET /v1/stats?group_by=tier` returns per-tier cost and performance breakdown
  5. Each request logs complexity score, matched tier, and selected provider at INFO level
**Plans:** 2/2 plans complete
Plans:
- [x] 20-01-PLAN.md -- Headers, SSE metadata, DB columns, and INFO logging for complexity routing
- [x] 20-02-PLAN.md -- Stats endpoint group_by=tier with per-tier cost/performance breakdown

## Progress

**Execution Order:**
Phases execute in numeric order: 16 -> 17 -> 18 -> 19 -> 20

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 16. Provider Tier Foundation | v1.7 | 2/2 | Complete    | 2026-04-08 |
| 17. Complexity Scorer | v1.7 | 1/1 | Complete    | 2026-04-08 |
| 18. Tier-Aware Routing | v1.7 | 1/1 | Complete    | 2026-04-08 |
| 19. Handler Integration and Escalation | v1.7 | 1/1 | Complete    | 2026-04-09 |
| 20. Routing Observability | v1.7 | 2/2 | Complete    | 2026-04-09 |
