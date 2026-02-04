# Milestone v1: Reliability and Observability

**Status:** SHIPPED 2026-02-04
**Phases:** 1-4
**Total Plans:** 10

## Overview

This milestone adds reliability and observability to arbstr's existing proxy. The work starts by fixing broken cost calculation and adding request correlation (foundation that everything else depends on), then builds out SQLite-backed request logging with accurate token and latency tracking, exposes per-request metadata to clients via response headers, and finally layers retry with fallback on top of the now-observable system. Each phase delivers a complete, verifiable capability.

## Phases

### Phase 1: Foundation
**Goal**: Every request has a correct cost calculation and a unique correlation ID for tracing
**Depends on**: Nothing (first phase)
**Requirements**: FNDTN-01, FNDTN-02
**Success Criteria**:
  1. Cost selection uses the full formula (input_tokens * input_rate + output_tokens * output_rate) / 1000 + base_fee, not just output_rate
  2. Every proxied request generates a unique correlation ID visible in structured logs
  3. Existing routing tests pass with the corrected cost formula (no regressions)

Plans:
- [x] 01-01-PLAN.md — Fix routing cost ranking (output_rate + base_fee) and add actual_cost_sats function (TDD)
- [x] 01-02-PLAN.md — Add per-request correlation IDs via TraceLayer make_span_with

### Phase 2: Request Logging
**Goal**: Every completed request is persistently logged with accurate token counts, costs, and latency
**Depends on**: Phase 1 (correct cost values and correlation IDs)
**Requirements**: OBSRV-01, OBSRV-02, OBSRV-03, OBSRV-04
**Success Criteria**:
  1. After proxying a non-streaming request, a row appears in SQLite with timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success, policy, and correlation ID
  2. Token counts in the log match the usage object returned by the provider
  3. Latency recorded reflects wall-clock time from request receipt to response completion
  4. SQLite writes never block the response to the client (async fire-and-forget)
  5. Database schema is applied automatically via embedded migrations on startup

Plans:
- [x] 02-01-PLAN.md — Storage infrastructure: migration SQL, storage module, pool init, RequestLog
- [x] 02-02-PLAN.md — Integration: register storage module, Database error variant, AppState.db (depends on 02-01)
- [x] 02-03-PLAN.md — Correlation ID in request extensions for handler access (depends on 02-02)
- [x] 02-04-PLAN.md — Request logging integration in chat_completions handler (depends on 02-02 + 02-03)

### Phase 3: Response Metadata
**Goal**: Clients can see per-request cost, latency, and correlation ID on every response
**Depends on**: Phase 2 (cost calculation, latency measurement, and correlation ID in place)
**Requirements**: OBSRV-05, OBSRV-06, OBSRV-07
**Success Criteria**:
  1. Non-streaming responses include an x-arbstr-cost-sats header with the actual cost in satoshis
  2. Non-streaming responses include an x-arbstr-latency-ms header with wall-clock latency
  3. All responses (streaming and non-streaming) include an x-arbstr-request-id header with the correlation ID
  4. Headers are visible to standard HTTP clients (curl, OpenAI SDK) without special configuration

Plans:
- [x] 03-01-PLAN.md — Add response metadata headers (constants, helper, restructured response paths, unit tests)

### Phase 4: Retry and Fallback
**Goal**: Failed requests are automatically retried and fall back to alternate providers without breaking API compatibility
**Depends on**: Phase 2 (logging captures retry attempts), Phase 3 (metadata headers in place for retry info)
**Requirements**: RLBTY-01, RLBTY-02, RLBTY-03, RLBTY-04, RLBTY-05
**Success Criteria**:
  1. A request that gets a 503 from the primary provider is retried with exponential backoff (up to 2 retries) and succeeds if the provider recovers
  2. After retries are exhausted on the primary provider, the request is forwarded to the next cheapest provider offering the same model
  3. The x-arbstr-retries header shows the number of attempts and which providers were tried
  4. Error responses through all retry and fallback paths remain valid OpenAI-compatible JSON with appropriate HTTP status codes
  5. Successful fallback requests are logged with the actual provider used, not the originally selected one

Plans:
- [x] 04-01-PLAN.md — Add select_candidates to Router for ordered provider list (RLBTY-03)
- [x] 04-02-PLAN.md — Create retry module with retry_with_fallback, backoff, and attempt tracking (RLBTY-01, RLBTY-02)
- [x] 04-03-PLAN.md — Wire retry into handler: timeout, idempotency key, x-arbstr-retries header (RLBTY-04, RLBTY-05)

## Progress

| Phase | Plans Complete | Status | Completed |
|-------|----------------|--------|-----------|
| 1. Foundation | 2/2 | Complete | 2026-02-02 |
| 2. Request Logging | 4/4 | Complete | 2026-02-04 |
| 3. Response Metadata | 1/1 | Complete | 2026-02-04 |
| 4. Retry and Fallback | 3/3 | Complete | 2026-02-04 |

---

## Milestone Summary

**Key Decisions:**
- Routing heuristic uses output_rate + base_fee (not full formula) since token counts unknown at selection time
- actual_cost_sats returns f64 for sub-satoshi precision
- UUID v4 generated internally by arbstr, not read from client headers
- Fire-and-forget logging: tokio::spawn, tracing::warn on failure
- Error path returns Ok(error_response) with arbstr headers instead of Err(Error)
- Streaming responses omit cost and latency headers (not known at header-send time)
- retry_with_fallback is generic with HasStatusCode trait, decoupled from handler types
- Arc<Mutex<Vec<AttemptRecord>>> for timeout-safe attempt tracking
- Streaming bypasses retry (no retry for streaming requests)

**Issues Resolved:**
- Cost calculation only used output_rate (broken) — fixed with full formula
- No request persistence — added SQLite logging
- Streaming errors were silent — acknowledged as v2 scope (RLBTY-06)
- No request correlation — added UUID v4 per request

**Issues Deferred:**
- Streaming token extraction (OBSRV-12) — deferred to v2
- Stream error handling (RLBTY-06) — deferred to v2
- Circuit breaker (RLBTY-07) — deferred to v2
- Per-provider timeouts (RLBTY-08) — deferred to v2

**Technical Debt Incurred:**
- BACKOFF_DURATIONS[2] (4s) defined but never reached at runtime
- default_strategy field retained with allow(dead_code)
- Pre-existing TODO comments for round_robin and lowest_latency strategies

---

_For current project status, see .planning/PROJECT.md_
_Archived: 2026-02-04_
