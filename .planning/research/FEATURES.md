# Feature Landscape: Streaming Observability in OpenAI-Compatible Proxies

**Domain:** SSE token extraction, post-stream logging, and streaming cost tracking for OpenAI-compatible proxy servers
**Researched:** 2026-02-15
**Overall confidence:** HIGH (well-documented OpenAI specification, established proxy patterns, existing codebase provides clear integration points)

## Current State Summary

arbstr already has full observability for non-streaming requests: token extraction from the `usage` JSON object, cost calculation via `actual_cost_sats()`, fire-and-forget SQLite logging, and response metadata headers (`x-arbstr-cost-sats`, `x-arbstr-latency-ms`). Streaming responses are forwarded as-is with a rudimentary chunk scanner that detects usage data but discards it. The log entry is written at request dispatch time (before the stream is consumed), so `input_tokens`, `output_tokens`, `cost_sats`, and `provider_cost_sats` are all `None` for every streaming request. The `x-arbstr-streaming: true` header signals to clients that cost data is unavailable.

This means streaming requests -- which represent a significant portion of real-world LLM usage -- are a cost-tracking blind spot. The data is passing through the proxy; it just is not captured or persisted.

---

## Table Stakes

Features users expect from any proxy that claims streaming observability. Missing these means the proxy's cost tracking has a known, permanent gap.

| Feature | Why Expected | Complexity | Depends On | Notes |
|---------|--------------|------------|------------|-------|
| **SSE usage extraction from final chunk** | OpenAI's API sends a final chunk with `usage: {prompt_tokens, completion_tokens, total_tokens}` when `stream_options: {"include_usage": true}` is set. Every observability-aware proxy (LiteLLM, OpenRouter, Azure API Management) captures this. Without it, streaming requests have no token counts. | Medium | Existing stream mapper in `handle_streaming_response` | The current code already has a chunk scanner that parses `data:` lines and detects usage. It logs a `tracing::debug!` but does not store the values. The mechanism exists; it needs to persist the captured data. |
| **Post-stream database UPDATE** | The log entry is currently written before the stream starts (fire-and-forget INSERT with `None` tokens). After the stream completes and usage is captured, the row must be updated with actual token counts and calculated cost. This is the only architecture that works because HTTP headers are sent before the stream body. | Medium | SSE usage extraction, existing SQLite infrastructure | Requires a new `UPDATE requests SET input_tokens=?, output_tokens=?, cost_sats=? WHERE correlation_id=?` query. The `correlation_id` is already indexed, so the update is fast. Must use `tokio::spawn` fire-and-forget like the initial INSERT. |
| **Inject `stream_options` into upstream request** | Not all providers send usage in streaming responses by default. OpenAI requires `stream_options: {"include_usage": true}` in the request body. arbstr should inject this automatically when streaming so usage data is available regardless of what the client requested. | Low | `ChatCompletionRequest` type in `types.rs` | Add `stream_options` field to `ChatCompletionRequest`. When `stream: true`, ensure `include_usage: true` is set before forwarding to the provider. This is the single most important enabler -- without it, usage capture is best-effort. |
| **SSE line buffering across chunk boundaries** | TCP segments do not align with SSE message boundaries. A `data: {"usage":...}` line can be split across two `bytes_stream()` chunks. The current code uses `text.lines()` on each chunk independently, which silently drops split lines. | Medium | SSE usage extraction | Requires maintaining a line buffer (`String`) across chunks. Append each chunk's bytes, split on `\n`, process complete lines, keep the trailing incomplete fragment for the next chunk. This is a well-known SSE parsing requirement. |
| **Latency reflects full stream duration** | Currently, `latency_ms` is captured at request dispatch time, not after the stream completes. For streaming, latency should reflect time-to-last-byte (full stream consumed), not time-to-first-byte. | Low | Post-stream database UPDATE | Capture `Instant::now()` at request start, compute elapsed after the stream wrapper detects completion (either `data: [DONE]` or stream close). Update the latency in the same post-stream UPDATE. |

### Implementation Priority for Table Stakes

1. **Add `stream_options` field to `ChatCompletionRequest` and auto-inject `include_usage: true`** -- This is the enabler. Without it, the provider may not send usage data at all. Low complexity, high leverage.
2. **SSE line buffering** -- Fix the chunk boundary bug in the existing stream mapper. Without correct buffering, usage extraction is unreliable under real network conditions.
3. **Capture usage from stream into shared state** -- Modify the existing stream mapper closure to store extracted usage in an `Arc<Mutex<Option<(u32, u32)>>>` (the pattern is already sketched in the phase-02 research).
4. **Post-stream database UPDATE** -- Add an `update_log_tokens` function to `storage/logging.rs` and trigger it from a `tokio::spawn` after the stream wrapper signals completion.
5. **Update latency in post-stream UPDATE** -- Piggyback on the post-stream UPDATE to also set the correct total latency.

---

## Differentiators

Features that go beyond basic observability. Not expected by all users, but signal a mature proxy that takes streaming seriously.

| Feature | Value Proposition | Complexity | Depends On | Notes |
|---------|-------------------|------------|------------|-------|
| **Streaming cost surfaced via trailing SSE event** | After the upstream `data: [DONE]`, inject an arbstr-specific SSE event like `data: {"arbstr_cost_sats": 42.35, "arbstr_latency_ms": 1523}` before the proxy's own `[DONE]`. Gives clients real-time cost visibility without a separate API call. Clients that do not understand arbstr events simply ignore the extra line. | Medium | SSE usage extraction, cost calculation | LiteLLM uses response headers (`x-litellm-response-cost`). OpenRouter includes cost in the final SSE chunk's `usage` object. Injecting a trailing SSE event is cleaner than headers (which are already sent) and does not modify the upstream response structure. Must be clearly documented as an arbstr extension. |
| **Output token counting by delta accumulation** | When usage is not available from the provider (no `stream_options` support, or interrupted stream), count output tokens by accumulating `delta.content` text length across all chunks. Use a rough 4-chars-per-token heuristic or model-specific tokenizer. Provides approximate cost even when the provider does not report usage. | Medium | SSE line buffering | LiteLLM uses tiktoken as a fallback. For Rust, `tiktoken-rs` crate exists but adds a large dependency. A simpler approach: count total characters in `delta.content` across all chunks, divide by 4 for an approximation. Mark as estimated in the log (`cost_estimated: true` flag). |
| **Time-to-first-token (TTFT) metric** | Record the elapsed time from request dispatch to the first non-empty `delta.content` chunk. This is a key latency metric for streaming UX and provider quality assessment. Store alongside total latency in the request log. | Low | SSE line buffering | Requires a boolean flag in the stream wrapper ("first content chunk seen?") and a timestamp comparison. Add `ttft_ms` column to the `requests` table. Azure API Management and Datadog both track TTFT as a first-class metric for LLM observability. |
| **Stream completion status tracking** | Distinguish between streams that completed normally (`data: [DONE]`), streams that were interrupted (connection dropped mid-stream), and streams that errored (provider sent an error chunk). Store the completion status in the request log. | Low | SSE line buffering | Add `stream_status` column: `completed`, `interrupted`, `error`. Currently, all streaming requests log as `success: true` regardless of whether the stream actually completed. This is a data quality issue. |
| **Streaming request retry preparation** | Currently, streaming requests bypass retry entirely ("cannot replay stream body"). With the stream wrapper intercepting chunks, arbstr could detect early failures (error in first chunk, empty stream, immediate disconnect) and retry before the client has received any data. Full mid-stream retry remains impossible, but early-failure retry is feasible. | High | Stream completion status tracking | Deferred to a later milestone. The current "fail fast" behavior is correct for most cases. Early-failure retry would help with transient provider errors that manifest immediately. |

### Differentiator Priority

1. **Time-to-first-token (TTFT) metric** -- Low complexity, high observability value. Add it during the stream wrapper implementation.
2. **Stream completion status tracking** -- Low complexity, fixes a real data quality problem (all streams currently logged as success).
3. **Streaming cost via trailing SSE event** -- Medium complexity, but gives clients immediate cost feedback that is currently unavailable. The `x-arbstr-streaming: true` header exists specifically because headers cannot carry this data.
4. **Output token counting by delta accumulation** -- Fallback for providers that do not support `stream_options`. Defer unless Routstr providers lack `include_usage` support.
5. **Streaming request retry preparation** -- High complexity, defer to a later milestone.

---

## Anti-Features

Features to deliberately NOT build for streaming observability.

| Anti-Feature | Why Other Products Have It | Why arbstr Should NOT Build It | What to Do Instead |
|--------------|---------------------------|-------------------------------|--------------------|
| **Full SSE parser with event types** | SSE spec defines `event:`, `id:`, `retry:` fields beyond `data:`. Full parsers handle reconnection, event IDs, and typed events. | OpenAI's streaming uses only `data:` lines. arbstr is a proxy, not an SSE client library. Parsing `event:` or `id:` lines adds complexity for data that is never present in OpenAI responses. | Use simple `strip_prefix("data: ")` on buffered lines. Skip non-data lines. The previous phase-02 research explicitly recommended this approach. |
| **Response body buffering for full-stream analysis** | Some observability tools buffer the entire streamed response in memory for post-hoc analysis (full token recount, content inspection). | Buffering defeats the purpose of streaming. It adds memory pressure proportional to response size and delays the proxy's stream forwarding. arbstr is a pass-through proxy; it should inspect but not buffer. | Extract usage from the final chunk (provider-reported), or accumulate delta lengths for approximation. Never buffer the full response. |
| **Client-side tokenizer for exact token counting** | LiteLLM bundles tiktoken for exact token counts when providers do not report them. | Adding `tiktoken-rs` or equivalent brings a large dependency (BPE vocabulary files, model-specific encoding tables). For a Rust binary that values small size and fast compile times, this is excessive. The provider-reported usage from `stream_options: {"include_usage": true}` is authoritative; client-side counting is a fallback for edge cases. | Rely on provider-reported usage. If unavailable, use character-count heuristic (4 chars/token) and flag as estimated. Add tokenizer only if Routstr providers consistently fail to report usage. |
| **Real-time streaming cost display in proxy logs** | Some dashboards show cost accumulating in real-time as tokens stream. | Real-time cost requires per-chunk output token counting (via tokenizer) and per-chunk cost calculation. This is overhead on every chunk for a metric that is only interesting in a dashboard arbstr does not have. | Calculate cost once after the stream completes, using the final usage data. Log it in the post-stream UPDATE. |
| **Modifying upstream response chunks** | Some proxies inject metadata (provider name, cost) into each streaming chunk. | Modifying upstream chunks breaks OpenAI compatibility. Clients expect unmodified `ChatCompletionChunk` objects. Injecting fields could cause client-side parse errors in strict OpenAI SDK implementations. | Inject arbstr metadata only as a separate trailing SSE event after `[DONE]`, or omit it entirely. Never modify upstream chunk JSON. |
| **Stream multiplexing / fan-out** | Some API gateways duplicate the stream to multiple consumers (client + logging pipeline). | Adds complexity (mpsc channels, backpressure management) for a single-user tool. The stream wrapper pattern (inspect-and-forward) achieves the same observability without duplication. | Use a single stream wrapper that inspects chunks as they pass through. The wrapper captures usage and signals completion via shared state. |

---

## Feature Dependencies

```
Add stream_options field to ChatCompletionRequest
  |
  +-- Auto-inject include_usage: true when stream=true
        |
        +-- Provider sends usage in final streaming chunk (prerequisite for all below)

SSE line buffering (fix chunk boundary handling)
  |
  +-- Reliable usage extraction from final chunk
  |     |
  |     +-- Store in Arc<Mutex<Option<(u32, u32)>>> shared with post-stream handler
  |           |
  |           +-- Post-stream database UPDATE (input_tokens, output_tokens, cost_sats)
  |           |     |
  |           |     +-- Total latency UPDATE (time-to-last-byte)
  |           |
  |           +-- Trailing SSE event with cost (optional differentiator)
  |
  +-- Stream completion detection (data: [DONE] vs connection drop)
  |     |
  |     +-- Stream completion status in log (completed/interrupted/error)
  |
  +-- Time-to-first-token detection (first non-empty delta.content)
        |
        +-- TTFT stored in request log

New migration: add ttft_ms, stream_status columns to requests table
```

Key dependency insight: **`stream_options` injection is the foundation.** Without `include_usage: true` in the upstream request, the provider may not send usage data, making all downstream extraction futile. This must ship first, even if the extraction and persistence logic comes later. The SSE line buffering fix is the second critical dependency -- without it, extraction is unreliable.

---

## MVP Recommendation

Based on the codebase analysis and ecosystem research, here is the recommended feature set for the v1.2 Streaming Observability milestone.

### Must Have (Table Stakes)

1. **`stream_options` field on `ChatCompletionRequest` with auto-injection** -- Add the field, auto-set `include_usage: true` when `stream: true`. This is zero-risk (additive field, does not change behavior if provider ignores it) and enables everything else. Estimated ~20 lines changed in `types.rs` and `handlers.rs`.

2. **SSE line buffering across chunk boundaries** -- Replace the naive `text.lines()` iteration in `handle_streaming_response` with a proper line buffer that accumulates across chunks. Without this, usage extraction fails silently under real network conditions. Estimated ~40 lines in `handlers.rs`.

3. **Capture streaming usage into shared state** -- Modify the stream wrapper closure to store `(prompt_tokens, completion_tokens)` in an `Arc<Mutex<Option<(u32, u32)>>>`. The current code already parses usage and logs it via `tracing::debug!`; it just needs to persist the value. Estimated ~15 lines changed.

4. **Post-stream database UPDATE** -- Add `update_streaming_log()` to `storage/logging.rs` that updates `input_tokens`, `output_tokens`, `cost_sats`, `provider_cost_sats`, and `latency_ms` for a given `correlation_id`. Trigger via `tokio::spawn` from a stream-completion callback. Estimated ~50 lines new.

5. **Stream completion callback** -- After the stream wrapper detects end-of-stream (`data: [DONE]` or connection close), execute a callback that reads the shared usage state, calculates cost via `actual_cost_sats()`, and triggers the post-stream UPDATE. This is the glue between extraction and persistence. Estimated ~40 lines.

### Should Have (Differentiators worth the effort)

6. **Time-to-first-token (TTFT) metric** -- Track first content chunk timestamp. Add `ttft_ms` column via migration. ~25 lines.

7. **Stream completion status** -- Track whether stream completed, was interrupted, or errored. Add `stream_status` column. ~20 lines.

8. **Trailing SSE event with cost data** -- Inject `data: {"arbstr_cost_sats": X, "arbstr_latency_ms": Y}\n\n` after the upstream stream completes but before the proxy closes the connection. ~30 lines.

### Defer

9. **Output token counting by delta accumulation** -- Only needed if Routstr providers do not support `stream_options`. Verify provider behavior first. If needed, add as a follow-up.

10. **Streaming early-failure retry** -- High complexity, unclear value for the common case. Defer to a later milestone.

---

## Complexity Estimates

| Feature | Lines of Code (est.) | New Files | Touches Existing | Risk |
|---------|---------------------|-----------|-----------------|------|
| `stream_options` field + auto-injection | ~20 | None | types.rs, handlers.rs (or send_to_provider) | Low -- additive field, providers ignore if unsupported |
| SSE line buffering | ~40 | None | handlers.rs (handle_streaming_response) | Medium -- must handle edge cases (empty chunks, multi-message chunks, split JSON) |
| Shared state usage capture | ~15 | None | handlers.rs (stream wrapper closure) | Low -- pattern already sketched in phase-02 research |
| Post-stream database UPDATE | ~50 | None | logging.rs (new function), handlers.rs (callback) | Low -- simple UPDATE query, correlation_id already indexed |
| Stream completion callback | ~40 | None | handlers.rs | Medium -- must detect both [DONE] and connection-drop completion |
| TTFT metric | ~25 | None | handlers.rs, logging.rs, new migration | Low -- boolean flag + timestamp diff |
| Stream completion status | ~20 | None | handlers.rs, logging.rs, new migration | Low -- enum column, set on stream end |
| Trailing SSE cost event | ~30 | None | handlers.rs (stream wrapper) | Medium -- must inject after [DONE] without breaking client SSE parsers |

**Total estimated new/changed code:** ~240 lines (table stakes only), ~315 lines (with differentiators)

**New migration required:** Yes, for `ttft_ms` and `stream_status` columns (if differentiators included).

---

## SSE Format Reference (OpenAI Specification)

For implementer reference, the exact SSE format arbstr must parse.

### Regular streaming chunks (content delivery)

```
data: {"id":"chatcmpl-abc","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":null}

```

### Final content chunk (finish_reason set)

```
data: {"id":"chatcmpl-abc","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":null}

```

### Usage chunk (when `stream_options.include_usage` is true)

```
data: {"id":"chatcmpl-abc","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":12,"total_tokens":21}}

```

Key observations:
- The usage chunk has an **empty `choices` array**, not null
- The `usage` field is **null on all chunks except the final usage chunk**
- The usage chunk appears **after the finish_reason chunk but before `[DONE]`**
- All fields (`prompt_tokens`, `completion_tokens`, `total_tokens`) are integers

### Stream termination

```
data: [DONE]

```

### Provider compatibility notes

| Provider | `stream_options.include_usage` | Notes |
|----------|-------------------------------|-------|
| OpenAI | Supported | Sends usage chunk before [DONE] |
| vLLM | Supported | Also supports `continuous_usage_stats` for per-chunk usage |
| Ollama | Supported | Added in recent versions, check compatibility |
| Routstr (api.routstr.com) | Unknown -- verify | Routstr wraps upstream providers; behavior depends on backend |
| Anthropic (native API) | Not applicable | Uses different API format, not OpenAI-compatible streaming |

**Confidence:** HIGH for OpenAI, MEDIUM for vLLM/Ollama (verified via documentation), LOW for Routstr (needs runtime verification).

---

## Sources and Confidence Notes

- **OpenAI API Reference - Chat Streaming** ([platform.openai.com/docs/api-reference/chat-streaming](https://platform.openai.com/docs/api-reference/chat-streaming)): HIGH confidence. Authoritative specification for `stream_options`, `include_usage`, and `ChatCompletionChunk` schema.
- **OpenAI Usage Stats Announcement** ([community.openai.com](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156)): HIGH confidence. Confirms `stream_options: {"include_usage": true}` feature and final chunk format with empty choices and populated usage.
- **OpenAI Streaming Packets Bug Report** ([community.openai.com](https://community.openai.com/t/bug-streaming-packets-changed/460882)): HIGH confidence. Documents real-world SSE chunk splitting across TCP boundaries, confirming the need for line buffering.
- **LiteLLM Token Usage and Cost Tracking** ([docs.litellm.ai/docs/completion/token_usage](https://docs.litellm.ai/docs/completion/token_usage)): MEDIUM confidence. Documents how LiteLLM handles streaming cost tracking, including known issues with inflated prompt_tokens during streaming (important cautionary data).
- **LiteLLM Streaming Token Bug** ([github.com/BerriAI/litellm/issues/12970](https://github.com/BerriAI/litellm/issues/12970)): MEDIUM confidence. Documents 10x-100x inflation in prompt_tokens when LiteLLM estimates tokens client-side during streaming -- validates the decision to prefer provider-reported usage over client-side tokenization.
- **vLLM OpenAI-Compatible Server** ([docs.vllm.ai/en/stable/serving/openai_compatible_server/](https://docs.vllm.ai/en/stable/serving/openai_compatible_server/)): MEDIUM confidence. Confirms vLLM supports `stream_options.include_usage` and `continuous_usage_stats`.
- **Ollama OpenAI Compatibility** ([docs.ollama.com/api/openai-compatibility](https://docs.ollama.com/api/openai-compatibility)): MEDIUM confidence. Lists `stream_options` as supported parameter.
- **OpenRouter Usage Accounting** ([openrouter.ai/docs/use-cases/usage-accounting](https://openrouter.ai/docs/use-cases/usage-accounting)): MEDIUM confidence. Confirms token usage in last SSE message for streaming responses.
- **Azure OpenAI Monitoring Architecture** ([learn.microsoft.com](https://learn.microsoft.com/en-us/azure/architecture/ai-ml/openai/architecture/log-monitor-azure-openai)): MEDIUM confidence. Enterprise-grade architecture for LLM proxy observability including streaming.
- **Adam Chalmers - Static Streams for Faster Async Proxies** ([blog.adamchalmers.com/streaming-proxy/](https://blog.adamchalmers.com/streaming-proxy/)): MEDIUM confidence. Rust-specific patterns for stream wrapping in async proxies.
- **arbstr Phase-02 Research** (`.planning/phases/02-request-logging/02-RESEARCH.md`): HIGH confidence. Internal documentation with code patterns for SSE chunk parsing, `Arc<Mutex>` usage capture, and identified pitfalls (chunk boundary misalignment).
