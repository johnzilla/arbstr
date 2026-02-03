# Domain Pitfalls: LLM Proxy Reliability and Observability

**Domain:** LLM routing proxy with Cashu token payments (Rust/Tokio/axum)
**Researched:** 2026-02-02
**Scope:** Provider fallback, streaming error recovery, request logging
**Confidence:** MEDIUM (based on codebase analysis and domain knowledge; web verification unavailable)

---

## Critical Pitfalls

Mistakes that cause data loss, financial loss, or require architectural rework.

---

### Pitfall 1: Double-Spend on Retry with Cashu Tokens

**What goes wrong:** When a provider request fails after the Cashu token has been submitted as an API key (Bearer token), retrying the same request sends the same token again. If the first request was partially processed (provider received the token, started generating, then failed mid-stream), the token may already be spent. The retry either fails (token already redeemed) or, worse, a second token is deducted if the system auto-generates a new one.

**Why it happens:** HTTP retry logic treats LLM requests as idempotent, but Cashu token redemption is a one-time operation. The current code in `handlers.rs` (line 63-65) sends `provider.api_key` as a Bearer token directly. There is no distinction between "connection failed before token was seen" and "provider received token, started work, then connection dropped."

**Consequences:**
- Users lose sats with no response
- Silent financial leakage that is hard to detect without cost tracking
- Trust erosion -- users will not use a proxy that loses their money

**Warning signs:**
- Retry logic that does not check whether the original request reached the provider
- No distinction between connection-level failures and application-level failures
- Missing cost reconciliation between expected and actual spend

**Prevention:**
1. Classify failures into "safe to retry" (DNS failure, connection refused, TLS handshake failure) vs "unsafe to retry" (timeout after request sent, mid-stream disconnect, 5xx after partial processing)
2. For unsafe-to-retry failures: do NOT retry automatically. Return the error to the client with context about what happened
3. If implementing automatic retry, use a fresh Cashu token for retries and mark the original token as "possibly spent" for reconciliation
4. Log every token submission attempt with a unique request ID, so cost tracking can detect double-spend
5. Consider a "pre-flight" health check to the provider before committing the token

**Detection:** Compare logged token submissions against provider-side receipts. If the provider exposes a usage/billing endpoint, poll it periodically. Track "requests sent" vs "responses completed" ratio -- divergence indicates lost tokens.

**Which phase should address it:** Must be designed in the retry/fallback phase. Cannot be bolted on later because retry logic that ignores this will already be causing financial loss.

---

### Pitfall 2: Silent Mid-Stream Failures in SSE Responses

**What goes wrong:** The current streaming implementation (handlers.rs lines 87-104) passes the upstream byte stream directly to the client. If the upstream connection drops mid-stream, the client receives a truncated response with no error indication. The SSE stream simply... stops. The client may interpret partial data as complete, especially if it does not validate the `[DONE]` sentinel.

**Why it happens:** SSE over HTTP/1.1 has no built-in framing for "stream ended abnormally." When `bytes_stream()` yields an error (line 90-93), the current code converts it to `std::io::Error`, which causes the stream to end -- but the client just sees the connection close. There is no `data: [DONE]` and no error event sent to the client. The `tracing::error!` on line 91 logs the issue server-side, but the client is left guessing.

**Consequences:**
- Client applications receive truncated LLM responses and may treat them as complete
- Code generation use cases produce broken/incomplete code
- No way for the client to distinguish "response complete" from "response failed mid-way"
- Cost is incurred for a response that was not fully delivered

**Warning signs:**
- Streaming handler that does not inject error events into the SSE stream
- No validation that `data: [DONE]` was received from upstream before closing the client stream
- Client-side tests that only test successful streaming, never mid-stream failures

**Prevention:**
1. Parse SSE events as they arrive (do not just pass raw bytes). Track whether `data: [DONE]` was received from the upstream provider
2. If the upstream stream errors before `[DONE]`, inject a synthetic SSE error event into the client stream: `data: {"error": {"message": "upstream connection lost", "type": "stream_error"}}`
3. Set a per-chunk timeout. If no data arrives for N seconds mid-stream, proactively send an error event and close
4. Track accumulated token count during streaming so cost logging has partial data even for failed streams
5. Consider buffering the first SSE event before committing to streaming -- if the first event is an error, you can return a proper HTTP error response instead of a broken stream

**Detection:** Monitor for streams that end without `[DONE]`. Track "streams started" vs "streams completed with [DONE]" ratio. Alert on divergence.

**Which phase should address it:** Streaming error handling should be in the same phase as retry/fallback, because the two interact (do you retry a failed stream? how much was already sent to the client?).

---

### Pitfall 3: SQLite Blocking the Tokio Runtime

**What goes wrong:** SQLite write operations hold a file-level lock. If request logging does a synchronous SQLite write (or an async write that blocks under contention), it stalls the Tokio worker thread. Under load, this causes all concurrent requests to hang -- not just the one doing the write.

**Why it happens:** SQLite is an embedded database with a single-writer model. Even with WAL mode, only one writer can proceed at a time. The sqlx crate (already in Cargo.toml) does use async I/O for SQLite, but under the hood it delegates to a blocking thread pool. If that pool is exhausted (default is often small), or if the write takes too long (disk I/O stall), backpressure propagates to the Tokio runtime. The most common mistake is logging synchronously in the request handler's hot path.

**Consequences:**
- Proxy latency spikes under load (tail latency goes from milliseconds to seconds)
- Request timeouts caused by the logging system, not the LLM provider
- Under extreme contention, the proxy becomes unresponsive

**Warning signs:**
- SQLite writes in the request handler's await chain (between receiving request and returning response)
- No WAL mode configured
- No connection pooling
- Database writes without a bounded channel/queue

**Prevention:**
1. Never await a database write in the request handler's critical path. Use a bounded async channel (e.g., `tokio::sync::mpsc`) to send log entries to a dedicated logging task
2. The logging task should batch writes (e.g., flush every 100ms or every 50 entries, whichever comes first)
3. Configure SQLite with WAL mode (`PRAGMA journal_mode=WAL`) and appropriate `busy_timeout` (`PRAGMA busy_timeout=5000`)
4. Use a single sqlx connection for writes (not a pool) to avoid lock contention between pool connections
5. If the channel is full (backpressure), drop log entries rather than blocking the request. Log a warning when entries are dropped. Cost tracking data is important but not worth stalling requests
6. Consider `spawn_blocking` for any synchronous SQLite operations

**Detection:** Monitor request latency percentiles (p99). If p99 is much higher than p50, suspect database contention. Add a timer around database operations and log when they exceed a threshold.

**Which phase should address it:** Must be designed correctly from the start of request logging implementation. Retrofitting async logging onto synchronous logging requires significant rework.

---

### Pitfall 4: Cost Calculation That Only Uses output_rate

**What goes wrong:** The current `select_cheapest` function (selector.rs line 170-172) sorts providers by `output_rate` only. This means a provider with `input_rate=100, output_rate=5` beats a provider with `input_rate=1, output_rate=10`, even though the first provider is far more expensive for prompts with large system messages or context. When request logging starts tracking actual costs, the logged costs will not match the selection rationale, making the cost data misleading.

**Why it happens:** Output tokens typically dominate cost for short-prompt/long-response use cases. But LLM usage increasingly involves large input contexts (RAG, code review, document analysis). The simplification was fine for MVP but becomes actively harmful once cost tracking relies on the same logic.

**Consequences:**
- Cost tracking reports inaccurate numbers
- Provider selection is suboptimal for high-input-token workloads
- Users see higher bills than expected because the "cheapest" provider was not actually cheapest for their usage pattern
- The `base_fee` field exists in config but is completely ignored in routing and cost calculation

**Warning signs:**
- Cost calculation that ignores any of the three rate components (input_rate, output_rate, base_fee)
- No input token estimation before provider selection
- Cost logging that records a single "cost" number without breaking down input/output/base components

**Prevention:**
1. Estimate total cost as: `(estimated_input_tokens * input_rate / 1000) + (estimated_output_tokens * output_rate / 1000) + base_fee`
2. For input tokens: count tokens from the request messages (use a tokenizer like tiktoken or approximate at 4 chars per token)
3. For output tokens before the response: use `max_tokens` if provided, otherwise use historical average from the token_ratios table (this is exactly what the planned `token_ratios` schema is for)
4. After the response: calculate actual cost from the `usage` field in the response and log the actual vs estimated delta
5. In cost logging, always store input_tokens, output_tokens, input_cost, output_cost, base_fee, and total_cost as separate columns

**Detection:** Compare estimated cost (at selection time) vs actual cost (from response usage). Large deltas indicate the estimation model is wrong. Track per-policy cost accuracy over time.

**Which phase should address it:** Fix cost calculation BEFORE implementing cost logging. If logging starts with broken cost numbers, the historical data is polluted and the learned token ratios will be wrong.

---

## Moderate Pitfalls

Mistakes that cause degraded experience, technical debt, or subtle bugs.

---

### Pitfall 5: Retry Without Backoff Creates Thundering Herd

**What goes wrong:** When the Routstr provider has a transient issue (rate limit, temporary overload), naive retry logic hammers it with immediate retries. With multiple concurrent users, all retries fire simultaneously, making the overload worse. The provider's rate limiter kicks in harder, causing more failures, causing more retries.

**Prevention:**
1. Use exponential backoff with jitter for retries (e.g., base 500ms, 1s, 2s, 4s with +/-25% random jitter)
2. Implement a circuit breaker pattern: after N consecutive failures to a provider, stop trying for a cooldown period. This is especially important because arbstr has a single Routstr endpoint -- if it is down, retrying just wastes the user's time
3. Set a maximum retry count (2-3 retries max for LLM requests, which are expensive)
4. Respect `Retry-After` headers from the provider if present
5. Return a meaningful error to the client when retries are exhausted, including how long they should wait

**Warning signs:**
- Retry logic with fixed delays or no delay
- No maximum retry count
- No circuit breaker
- Retry count not exposed in metrics/logs

**Which phase should address it:** Retry/fallback implementation phase. Design the backoff strategy before writing the retry loop.

---

### Pitfall 6: Logging Request Bodies Creates Storage and Privacy Bombs

**What goes wrong:** Teams implementing request logging start by logging full request/response bodies "for debugging." For LLM proxies, this means storing entire conversation histories, system prompts (which may contain proprietary instructions), and full LLM responses. Storage grows rapidly (a single request can be 100KB+), and the logs contain sensitive user data.

**Prevention:**
1. Log metadata only by default: model, provider, token counts, cost, latency, policy, success/failure, timestamp
2. If body logging is needed for debugging, make it opt-in via config (`log_request_bodies = false` by default)
3. When body logging is enabled, truncate to a maximum length and redact known sensitive patterns
4. Set up database size monitoring and automatic cleanup (e.g., retain 30 days of metadata, 7 days of bodies)
5. The planned `requests` table schema in CLAUDE.md is correct -- it logs metadata, not bodies. Stick to that design

**Warning signs:**
- Request/response body columns in the logging table
- No database size limits or retention policy
- No config option to control logging verbosity

**Which phase should address it:** Request logging implementation phase. Decide the logging granularity at design time.

---

### Pitfall 7: Streaming Token Counting Is Harder Than Non-Streaming

**What goes wrong:** For non-streaming responses, the `usage` field in the JSON response gives exact token counts. For streaming responses, the `usage` field is only present in the final chunk (if the provider includes it at all -- not all do). If the code does not parse SSE events during streaming, it has no way to extract token counts, making cost logging impossible for streaming requests.

**Why it happens:** The current streaming handler (handlers.rs lines 87-104) passes raw bytes through without parsing them. This is efficient but means arbstr has zero visibility into what was streamed.

**Consequences:**
- Streaming requests have no cost data in the database
- Cost dashboard shows costs only for non-streaming requests, which may be a minority of traffic
- Token ratio learning (the planned `token_ratios` table) is trained on a biased sample

**Prevention:**
1. Parse SSE events during streaming. Each `data: {...}` line can be deserialized to `ChatCompletionChunk`. Count `delta.content` characters and estimate tokens
2. Watch for the final chunk which may contain a `usage` field. If present, use it as ground truth. If not, use the character-based estimate
3. Buffer the parsed events only for metadata extraction -- still forward the raw bytes to the client for minimum latency
4. Consider a dual-path approach: tee the stream into a parser task and the client response simultaneously, so parsing never delays delivery
5. Track which providers include `usage` in streaming responses and which do not, so cost accuracy is per-provider

**Warning signs:**
- Streaming handler that does not parse SSE events
- Cost logging table with NULL token counts for streaming requests
- Cost dashboard that ignores streaming requests

**Which phase should address it:** Implement SSE parsing in the streaming error handling phase, before cost logging. The parser serves double duty: error detection AND cost extraction.

---

### Pitfall 8: Fallback to Different Model Breaks Client Expectations

**What goes wrong:** When the requested model is unavailable or the provider fails, "fallback" logic routes to a different model (e.g., user requests `gpt-4o`, fallback sends to `gpt-4o-mini`). The client receives a response from a model it did not request, with different capabilities, token limits, and cost. Applications that depend on specific model behavior (structured output, function calling, specific knowledge cutoff) break silently.

**Prevention:**
1. Default to same-model retry only. Never silently switch models
2. If model fallback is desired, make it opt-in via policy config (e.g., `fallback_models = ["gpt-4o-mini"]` under a policy rule)
3. When model fallback occurs, include it in the response headers (`X-Arbstr-Fallback-Model: gpt-4o-mini`) and in the response body metadata
4. Document that model fallback is a conscious decision, not an automatic behavior
5. The `model` field in the response should reflect the actual model used, not the requested model

**Warning signs:**
- Fallback logic that changes the model without the client's knowledge
- No header or metadata indicating a fallback occurred
- Tests that verify fallback works but do not verify the client is informed

**Which phase should address it:** Retry/fallback phase. Define fallback behavior as part of the policy configuration, not as implicit behavior.

---

### Pitfall 9: Request Timeout That Kills Long-Running Completions

**What goes wrong:** The current HTTP client has a 120-second timeout (server.rs line 51). For large context completions (100K+ token inputs with models that support it), 120 seconds may not be enough. The timeout kills a legitimate request, the user gets an error, but the provider has already processed the tokens and charged for them.

**Prevention:**
1. Separate connect timeout (short, e.g., 10 seconds -- already done) from response timeout (long or per-request)
2. For streaming requests, the timeout should be a per-chunk idle timeout, not a total timeout. If data is flowing, the stream should continue indefinitely
3. Make the total timeout configurable per-policy (code generation may need 5 minutes, quick chat may need 30 seconds)
4. When a timeout fires, log it clearly and do NOT retry (the provider already processed the request)
5. Consider allowing clients to set their own timeout via a header (`X-Arbstr-Timeout: 300`)

**Warning signs:**
- Single global timeout for all request types
- Streaming requests using the same timeout as non-streaming
- Timeout errors with no logged context about what was lost

**Which phase should address it:** Retry/fallback phase. Timeout configuration is a prerequisite for correct retry behavior.

---

## Minor Pitfalls

Mistakes that cause confusion or minor degradation but are easily fixed.

---

### Pitfall 10: OpenAI Error Format Inconsistency

**What goes wrong:** The current error handler (error.rs) returns errors in a format close to but not identical to OpenAI's error format. OpenAI uses `{"error": {"message": "...", "type": "...", "param": null, "code": "..."}}` where `code` is a string like `"model_not_found"`, not an HTTP status number. Client libraries that parse error codes as strings will break on arbstr's numeric codes.

**Prevention:**
1. Match OpenAI's exact error format: `code` should be a string like `"invalid_request_error"`, not a number like `400`
2. Include the `param` field (can be `null`) for compatibility
3. Map arbstr errors to OpenAI error types: `NoProviders` -> `model_not_found`, `BadRequest` -> `invalid_request_error`, `Provider` -> `server_error`
4. Test error responses against the official OpenAI Python and Node.js client libraries

**Warning signs:**
- Error response format that was not tested against real OpenAI client libraries
- Numeric `code` field instead of string

**Which phase should address it:** Can be fixed independently, but ideally before request logging (so logged errors have correct types).

---

### Pitfall 11: Health Check That Does Not Check Provider Connectivity

**What goes wrong:** The current `/health` endpoint (handlers.rs lines 155-160) returns `{"status": "ok"}` unconditionally. It does not check whether the proxy can actually reach its configured providers. Load balancers and monitoring systems that rely on the health check will think the proxy is healthy even when all providers are unreachable.

**Prevention:**
1. Add a `/health/ready` endpoint (readiness probe) that checks provider connectivity
2. Keep `/health` as a liveness probe (is the process running?)
3. Cache the readiness check result (e.g., check every 30 seconds) to avoid hammering providers on every health check
4. Include provider status in the readiness response: `{"status": "degraded", "providers": {"alpha": "ok", "beta": "unreachable"}}`

**Warning signs:**
- Health endpoint that returns static `ok`
- No distinction between liveness and readiness
- Monitoring that shows "healthy" when users report errors

**Which phase should address it:** Observability phase, alongside metrics and logging.

---

### Pitfall 12: Tracing Without Request Correlation IDs

**What goes wrong:** The current logging uses `tracing::info!` with individual fields but does not assign a unique request ID that persists across all log entries for a single request. When debugging under load, it is impossible to correlate "Received chat completion request" with "Selected provider" with "Error streaming from provider" for a specific request.

**Prevention:**
1. Generate a UUID (already in Cargo.toml) at the start of each request handler
2. Use `tracing::info_span!` with the request ID to create a span that covers the entire request lifecycle
3. Include the request ID in response headers (`X-Request-Id`) so clients can reference it in bug reports
4. If the client sends an `X-Request-Id` header, use it (for end-to-end tracing)
5. Include the request ID in database log entries

**Warning signs:**
- Log entries without a request ID
- Unable to trace a single request through the log
- Using `tracing::info!` instead of spans for request lifecycle

**Which phase should address it:** Should be the first thing implemented in the observability phase. All subsequent logging and metrics benefit from correlation IDs.

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|---|---|---|
| Retry/Fallback | Double-spend on retry with Cashu tokens (Pitfall 1) | Classify failures by safety-to-retry. Never auto-retry after token submission without fresh token |
| Retry/Fallback | Thundering herd from immediate retries (Pitfall 5) | Exponential backoff with jitter, circuit breaker |
| Retry/Fallback | Model switching without client consent (Pitfall 8) | Same-model-only by default, opt-in fallback via policy |
| Retry/Fallback | Timeout killing legitimate long completions (Pitfall 9) | Per-chunk idle timeout for streaming, per-policy total timeout |
| Streaming Error Handling | Silent mid-stream failures (Pitfall 2) | Parse SSE events, inject error events, validate `[DONE]` |
| Streaming Error Handling | No token counting for streams (Pitfall 7) | Parse SSE during streaming for metadata extraction |
| Request Logging | SQLite blocking async runtime (Pitfall 3) | Async channel to dedicated writer task, WAL mode, batched writes |
| Request Logging | Logging request bodies (Pitfall 6) | Metadata-only by default, opt-in body logging with truncation |
| Cost Tracking | Broken cost calculation (Pitfall 4) | Fix cost formula BEFORE logging starts. Include all three rate components |
| Observability | No request correlation IDs (Pitfall 12) | Add span-based request IDs as first observability step |
| Observability | Health check is useless (Pitfall 11) | Separate liveness and readiness probes |
| API Compatibility | Error format mismatch (Pitfall 10) | Match OpenAI error format exactly, test against real client libraries |

## Recommended Phase Ordering Based on Pitfalls

The pitfall dependencies suggest this ordering:

1. **Fix cost calculation** (Pitfall 4) -- prerequisite for everything else. If cost numbers are wrong, all downstream logging and learning is polluted
2. **Add request correlation IDs** (Pitfall 12) -- prerequisite for debugging everything else
3. **Implement SSE stream parsing** (Pitfalls 2, 7) -- needed for both error handling and cost extraction from streams
4. **Implement request logging** (Pitfalls 3, 6) -- with correct cost calculation, correlation IDs, and stream token counts already in place
5. **Implement retry/fallback** (Pitfalls 1, 5, 8, 9) -- last because it is the most complex and benefits from all the observability built in prior steps. You need logging to validate that retry logic is working correctly

## Existing Code Vulnerabilities (Direct Observations)

These are not hypothetical pitfalls but actual issues observed in the codebase:

| File | Line(s) | Issue |
|---|---|---|
| `src/proxy/handlers.rs` | 87-104 | Streaming passes raw bytes with no SSE parsing, no error injection, no `[DONE]` validation |
| `src/proxy/handlers.rs` | 67-70 | Provider failure returns error but does not distinguish retryable vs non-retryable |
| `src/proxy/handlers.rs` | 120 | `TODO: Log to database for cost tracking` -- logging not implemented |
| `src/router/selector.rs` | 170-172 | `select_cheapest` uses only `output_rate`, ignores `input_rate` and `base_fee` |
| `src/proxy/server.rs` | 51 | Single 120s timeout for all requests, no per-chunk timeout for streaming |
| `src/error.rs` | 47-53 | Error `code` is numeric HTTP status, not OpenAI string code |
| `src/proxy/handlers.rs` | 155-160 | Health check is unconditional `ok`, does not check provider reachability |

## Sources

- Direct analysis of codebase at `/home/john/projects/github.com/arbstr/src/`
- Domain knowledge of SSE protocol, SQLite concurrency model, OpenAI API format, Cashu token properties
- Confidence: MEDIUM overall. Pitfalls 1-4 are HIGH confidence (directly observable in code or inherent to the technology). Pitfalls 5-12 are MEDIUM confidence (common patterns, but specific impact depends on usage scale). Web-based verification was not available to cross-reference community experiences.
