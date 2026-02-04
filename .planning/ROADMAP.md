# Roadmap: arbstr

## Overview

This milestone adds reliability and observability to arbstr's existing proxy. The work starts by fixing broken cost calculation and adding request correlation (foundation that everything else depends on), then builds out SQLite-backed request logging with accurate token and latency tracking, exposes per-request metadata to clients via response headers, and finally layers retry with fallback on top of the now-observable system. Each phase delivers a complete, verifiable capability.

## Phases

**Phase Numbering:**
- Integer phases (1, 2, 3, 4): Planned milestone work
- Decimal phases (2.1, 2.2): Urgent insertions (marked with INSERTED)

Decimal phases appear between their surrounding integers in numeric order.

- [x] **Phase 1: Foundation** - Fix cost calculation and add request correlation IDs
- [ ] **Phase 2: Request Logging** - SQLite storage with async request logging, token extraction, and latency tracking
- [ ] **Phase 3: Response Metadata** - Expose cost, latency, and correlation ID to clients via response headers
- [ ] **Phase 4: Retry and Fallback** - Retry failed requests with backoff and fall back to alternate providers

## Phase Details

### Phase 1: Foundation
**Goal**: Every request has a correct cost calculation and a unique correlation ID for tracing
**Depends on**: Nothing (first phase)
**Requirements**: FNDTN-01, FNDTN-02
**Success Criteria** (what must be TRUE):
  1. Cost selection uses the full formula (input_tokens * input_rate + output_tokens * output_rate) / 1000 + base_fee, not just output_rate
  2. Every proxied request generates a unique correlation ID visible in structured logs
  3. Existing routing tests pass with the corrected cost formula (no regressions)
**Plans**: 2 plans

Plans:
- [x] 01-01-PLAN.md — Fix routing cost ranking (output_rate + base_fee) and add actual_cost_sats function (TDD)
- [x] 01-02-PLAN.md — Add per-request correlation IDs via TraceLayer make_span_with

### Phase 2: Request Logging
**Goal**: Every completed request is persistently logged with accurate token counts, costs, and latency
**Depends on**: Phase 1 (correct cost values and correlation IDs)
**Requirements**: OBSRV-01, OBSRV-02, OBSRV-03, OBSRV-04
**Success Criteria** (what must be TRUE):
  1. After proxying a non-streaming request, a row appears in SQLite with timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success, policy, and correlation ID
  2. Token counts in the log match the usage object returned by the provider
  3. Latency recorded reflects wall-clock time from request receipt to response completion
  4. SQLite writes never block the response to the client (async fire-and-forget)
  5. Database schema is applied automatically via embedded migrations on startup
**Plans**: 4 plans

Plans:
- [x] 02-01-PLAN.md — Storage infrastructure: migration SQL, storage module, pool init, RequestLog
- [x] 02-02-PLAN.md — Integration: register storage module, Database error variant, AppState.db (depends on 02-01)
- [ ] 02-03-PLAN.md — Correlation ID in request extensions for handler access (depends on 02-02)
- [ ] 02-04-PLAN.md — Request logging integration in chat_completions handler (depends on 02-02 + 02-03)

### Phase 3: Response Metadata
**Goal**: Clients can see per-request cost, latency, and correlation ID on every response
**Depends on**: Phase 2 (cost calculation, latency measurement, and correlation ID in place)
**Requirements**: OBSRV-05, OBSRV-06, OBSRV-07
**Success Criteria** (what must be TRUE):
  1. Non-streaming responses include an x-arbstr-cost-sats header with the actual cost in satoshis
  2. Non-streaming responses include an x-arbstr-latency-ms header with wall-clock latency
  3. All responses (streaming and non-streaming) include an x-arbstr-request-id header with the correlation ID
  4. Headers are visible to standard HTTP clients (curl, OpenAI SDK) without special configuration
**Plans**: TBD

Plans:
- [ ] 03-01: TBD

### Phase 4: Retry and Fallback
**Goal**: Failed requests are automatically retried and fall back to alternate providers without breaking API compatibility
**Depends on**: Phase 2 (logging captures retry attempts), Phase 3 (metadata headers in place for retry info)
**Requirements**: RLBTY-01, RLBTY-02, RLBTY-03, RLBTY-04, RLBTY-05
**Success Criteria** (what must be TRUE):
  1. A request that gets a 503 from the primary provider is retried with exponential backoff (up to 2 retries) and succeeds if the provider recovers
  2. After retries are exhausted on the primary provider, the request is forwarded to the next cheapest provider offering the same model
  3. The x-arbstr-retries header shows the number of attempts and which providers were tried
  4. Error responses through all retry and fallback paths remain valid OpenAI-compatible JSON with appropriate HTTP status codes
  5. Successful fallback requests are logged with the actual provider used, not the originally selected one
**Plans**: TBD

Plans:
- [ ] 04-01: TBD
- [ ] 04-02: TBD
- [ ] 04-03: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 1 -> 2 -> 3 -> 4

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Foundation | 2/2 | Complete ✓ | 2026-02-02 |
| 2. Request Logging | 2/4 | In progress | - |
| 3. Response Metadata | 0/TBD | Not started | - |
| 4. Retry and Fallback | 0/TBD | Not started | - |
