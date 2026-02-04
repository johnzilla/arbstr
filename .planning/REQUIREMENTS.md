# Requirements: arbstr

**Defined:** 2026-02-02
**Core Value:** Smart model selection that minimizes sats spent per request without sacrificing quality

## v1 Requirements

### Foundation

- [x] **FNDTN-01**: Cost calculation uses full formula: (input_tokens * input_rate + output_tokens * output_rate) / 1000 + base_fee
- [x] **FNDTN-02**: Each request assigned a unique correlation ID for tracing

### Observability

- [x] **OBSRV-01**: Every completed request logged to SQLite with timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success, policy name, correlation ID
- [x] **OBSRV-02**: Token counts extracted from non-streaming provider responses (usage object)
- [x] **OBSRV-03**: Latency measured as wall-clock time from request receipt to response completion
- [x] **OBSRV-04**: SQLite writes are async (fire-and-forget via tokio::spawn), never blocking the response path
- [x] **OBSRV-05**: Non-streaming responses include x-arbstr-cost-sats header with actual cost
- [x] **OBSRV-06**: Non-streaming responses include x-arbstr-latency-ms header
- [x] **OBSRV-07**: Responses include x-arbstr-request-id header with correlation ID

### Reliability

- [x] **RLBTY-01**: Failed requests (429, 500, 502, 503, 504) retried with exponential backoff, max 2 retries
- [x] **RLBTY-02**: After retries exhausted on primary provider, request falls back to next cheapest provider for same model
- [x] **RLBTY-03**: Router returns an ordered list of candidate providers, not just the top pick
- [x] **RLBTY-04**: Retry/fallback metadata (attempts, providers tried) included in response headers (x-arbstr-retries)
- [x] **RLBTY-05**: Error responses remain OpenAI-compatible through all retry/fallback paths

## v2 Requirements

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

## Out of Scope

| Feature | Reason |
|---------|--------|
| Web dashboard UI | Single user, CLI/curl sufficient. Adds frontend complexity for no value. |
| Client authentication to arbstr | Running on home network, only user. No abuse vector. |
| Rate limiting | Single user, no abuse vector. |
| Prompt caching | Single-user usage rarely produces duplicate prompts. High complexity. |
| Cross-model fallback | Silently substituting cheaper model changes quality. Fallback is same-model only. |
| Cashu wallet management | Balance monitored externally at the mint. |
| Guardrails / content filtering | Personal tool, user responsible for own prompts. |
| Real-time streaming analytics | Complexity far exceeds value for single user. |

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
- Mapped to phases: 14
- Unmapped: 0

---
*Requirements defined: 2026-02-02*
*Last updated: 2026-02-04 after Phase 4 completion*
