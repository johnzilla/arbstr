# Requirements Archive: v1 Reliability and Observability

**Archived:** 2026-02-04
**Status:** SHIPPED

This is the archived requirements specification for v1.
For current requirements, see `.planning/REQUIREMENTS.md` (created for next milestone).

---

**Defined:** 2026-02-02
**Core Value:** Smart model selection that minimizes sats spent per request without sacrificing quality

## v1 Requirements

### Foundation

- [x] **FNDTN-01**: Cost calculation uses full formula: (input_tokens * input_rate + output_tokens * output_rate) / 1000 + base_fee
  - *Outcome: Implemented as actual_cost_sats returning f64 for sub-sat precision*
- [x] **FNDTN-02**: Each request assigned a unique correlation ID for tracing
  - *Outcome: UUID v4 via TraceLayer make_span_with, visible at default log level*

### Observability

- [x] **OBSRV-01**: Every completed request logged to SQLite with timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success, policy name, correlation ID
  - *Outcome: RequestLog struct with 14 fields, parameterized INSERT, all code paths logged*
- [x] **OBSRV-02**: Token counts extracted from non-streaming provider responses (usage object)
  - *Outcome: extract_usage function with 4 unit tests, streaming tokens deferred to v2*
- [x] **OBSRV-03**: Latency measured as wall-clock time from request receipt to response completion
  - *Outcome: Instant::now at handler start, elapsed().as_millis() before logging*
- [x] **OBSRV-04**: SQLite writes are async (fire-and-forget via tokio::spawn), never blocking the response path
  - *Outcome: spawn_log_write with tracing::warn on failure*
- [x] **OBSRV-05**: Non-streaming responses include x-arbstr-cost-sats header with actual cost
  - *Outcome: Formatted with 2 decimal places via centralized attach_arbstr_headers helper*
- [x] **OBSRV-06**: Non-streaming responses include x-arbstr-latency-ms header
  - *Outcome: Included on non-streaming success and error responses*
- [x] **OBSRV-07**: Responses include x-arbstr-request-id header with correlation ID
  - *Outcome: Included on all responses (streaming, non-streaming, error)*

### Reliability

- [x] **RLBTY-01**: Failed requests (429, 500, 502, 503, 504) retried with exponential backoff, max 2 retries
  - *Outcome: is_retryable checks 5xx codes, 1s/2s backoff, non-streaming only*
- [x] **RLBTY-02**: After retries exhausted on primary provider, request falls back to next cheapest provider for same model
  - *Outcome: retry_with_fallback with candidates array, single fallback attempt*
- [x] **RLBTY-03**: Router returns an ordered list of candidate providers, not just the top pick
  - *Outcome: select_candidates returns Vec<SelectedProvider> sorted by routing cost*
- [x] **RLBTY-04**: Retry/fallback metadata (attempts, providers tried) included in response headers (x-arbstr-retries)
  - *Outcome: format_retries_header produces "N/provider-name" format, attached on all retried paths*
- [x] **RLBTY-05**: Error responses remain OpenAI-compatible through all retry/fallback paths
  - *Outcome: Error::into_response() used throughout, 504 override for timeout*

## v2 Requirements (Deferred)

### Observability (deferred)

- **OBSRV-08**: Cost query endpoint (GET /costs with period and model/policy grouping)
- **OBSRV-09**: Per-model and per-policy cost breakdown queries
- **OBSRV-10**: Enhanced /health endpoint with per-provider status and success rates
- **OBSRV-11**: Learned token ratios per policy for predictive cost estimation
- **OBSRV-12**: Token counts extracted from streaming responses (SSE parsing or stream_options)

### Reliability (deferred)

- **RLBTY-06**: Stream error handling (detect mid-stream failures, signal client with clean SSE error event)
- **RLBTY-07**: Circuit breaker per provider (stop sending after N consecutive failures, cooldown period)
- **RLBTY-08**: Per-provider timeout configuration (replace global 120s)

## Traceability

| Requirement | Phase | Status |
|-------------|-------|--------|
| FNDTN-01 | Phase 1: Foundation | Complete |
| FNDTN-02 | Phase 1: Foundation | Complete |
| OBSRV-01 | Phase 2: Request Logging | Complete |
| OBSRV-02 | Phase 2: Request Logging | Complete |
| OBSRV-03 | Phase 2: Request Logging | Complete |
| OBSRV-04 | Phase 2: Request Logging | Complete |
| OBSRV-05 | Phase 3: Response Metadata | Complete |
| OBSRV-06 | Phase 3: Response Metadata | Complete |
| OBSRV-07 | Phase 3: Response Metadata | Complete |
| RLBTY-01 | Phase 4: Retry and Fallback | Complete |
| RLBTY-02 | Phase 4: Retry and Fallback | Complete |
| RLBTY-03 | Phase 4: Retry and Fallback | Complete |
| RLBTY-04 | Phase 4: Retry and Fallback | Complete |
| RLBTY-05 | Phase 4: Retry and Fallback | Complete |

**Coverage:**
- v1 requirements: 14 total
- Shipped: 14
- Adjusted: 0
- Dropped: 0

---

## Milestone Summary

**Shipped:** 14 of 14 v1 requirements
**Adjusted:** None — all requirements delivered as specified
**Dropped:** None

---
*Archived: 2026-02-04 as part of v1 milestone completion*
