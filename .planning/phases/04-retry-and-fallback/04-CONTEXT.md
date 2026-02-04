# Phase 4: Retry and Fallback - Context

**Gathered:** 2026-02-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Automatically retry failed non-streaming requests with exponential backoff and fall back to an alternate provider when the primary fails. All retry/fallback behavior is transparent to the client via response headers. Streaming requests are not retried. No new endpoints, no config options beyond existing provider definitions.

</domain>

<decisions>
## Implementation Decisions

### Retry trigger conditions
- Retry on 5xx server errors only (500, 502, 503, 504)
- Do NOT retry client errors (4xx) -- these are permanent failures
- Fixed exponential backoff: 1s, 2s, 4s (no jitter)
- Maximum 2 retries per provider (3 total attempts on primary)
- 30-second total timeout for the entire retry+fallback chain -- if not resolved within 30s, fail
- Non-streaming requests only -- streaming requests fail fast, no retry

### Fallback provider policy
- After retries exhausted on primary: fall back to next cheapest same-model provider
- One fallback provider only (2 providers total: primary + one fallback)
- Fallback gets one shot only (no retries on fallback)
- Fallback must be a different provider name (skip same provider even if listed multiple times)
- If no alternate same-model provider exists, return error immediately after primary retries exhaust

### Cashu token handling
- Re-send same api_key on retry -- assume session-based auth (sk- key), not one-time Cashu tokens
- If this assumption is wrong, retry will fail with a 4xx which won't trigger further retries (safe failure mode)
- Send existing correlation ID (x-arbstr-request-id) as idempotency key to provider via request header to prevent double-charge on race conditions
- Double-charge risk on race conditions mitigated by idempotency key; if provider doesn't support it, low-impact for single-user proxy

### Error & header semantics
- x-arbstr-retries header format: compact count per provider, e.g. "2/provider-alpha, 1/provider-beta"
- x-arbstr-retries present on ALL responses that involved retries (success and failure) -- clients can monitor retry frequency
- On total failure: return last provider's HTTP status and error body, enhanced with x-arbstr-retries header showing full attempt history
- x-arbstr-provider header shows actual provider that handled the request (fallback provider if primary failed)
- Log only final outcome in SQLite (one row per original request), not individual retry attempts -- provider field shows actual provider used

### Claude's Discretion
- Router API changes to return ordered candidate list (how to surface alternates for fallback)
- Where to place retry loop (handler level vs execute_request level vs new retry module)
- Idempotency key header name (e.g. X-Idempotency-Key or Idempotency-Key)
- How to track attempt history for the x-arbstr-retries header value construction

</decisions>

<specifics>
## Specific Ideas

No specific requirements -- open to standard approaches.

</specifics>

<deferred>
## Deferred Ideas

- Circuit breaker per provider (stop sending after N consecutive failures) -- RLBTY-07, deferred to v2
- Per-provider timeout configuration -- RLBTY-08, deferred to v2
- Stream error handling and retry -- RLBTY-06, deferred to v2
- 429 rate-limit retry with Retry-After header support -- could add later if providers return 429s

</deferred>

---

*Phase: 04-retry-and-fallback*
*Context gathered: 2026-02-03*
