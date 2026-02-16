# Domain Pitfalls: SSE Stream Interception and Token Extraction

**Domain:** Adding SSE stream interception to an existing Rust/Tokio/axum pass-through proxy
**Researched:** 2026-02-15
**Scope:** Stream parsing, buffering, provider format differences, database update patterns, memory/latency concerns
**Confidence:** HIGH (based on direct codebase analysis + verified SSE specification + provider documentation + community reports)

---

## Critical Pitfalls

Mistakes that break existing streaming, lose data, or require architectural rework.

---

### Pitfall 1: SSE Data Lines Split Across TCP Chunk Boundaries

**What goes wrong:** The current `handle_streaming_response` (handlers.rs:665-729) receives bytes from `upstream_response.bytes_stream()` and processes each chunk independently with `text.lines()`. This assumes that each `Bytes` chunk from reqwest contains complete SSE lines. In reality, TCP delivers data in arbitrarily-sized chunks. A single SSE event like `data: {"choices":[{"delta":{"content":"hello"}}]}\n\n` can be split across two or more `Bytes` chunks at any byte boundary, including:

- Mid-line: chunk 1 = `data: {"choices":[{"del`, chunk 2 = `ta":{"content":"hello"}}]}\n\n`
- Mid-prefix: chunk 1 = `dat`, chunk 2 = `a: {"choices":...}\n\n`
- Mid-JSON: chunk 1 = `data: {"usage":{"prompt_tokens":100,"completion_to`, chunk 2 = `kens":200}}\n\n`

The current code calls `text.lines()` on each chunk, which produces partial lines that fail `strip_prefix("data: ")` or produce invalid JSON that `serde_json::from_str` silently rejects. The usage data from the final chunk -- the entire purpose of this feature -- is the most likely to be split because it arrives alongside `data: [DONE]` and the TCP FIN.

**Why it happens:** `reqwest::Response::bytes_stream()` yields `Bytes` chunks corresponding to HTTP chunked transfer encoding boundaries or TCP segment boundaries, NOT SSE event boundaries. Developers test with localhost where chunks tend to be large and aligned, then discover splits in production over real networks with varying MTU, Nagle's algorithm, and intermediary proxies.

**Consequences:**
- Usage data silently dropped for a percentage of requests (the final chunk is most vulnerable to splitting)
- Token counts and cost tracking become unreliable with no obvious error
- Tests pass locally but fail in production under real network conditions

**Warning signs:**
- `.lines()` called on raw `Bytes` without cross-chunk buffering
- No test for SSE events split across chunk boundaries
- Usage extraction that works in local testing but shows NULL in production logs

**How to avoid:**
1. Maintain a `String` buffer across chunks. Append each chunk's bytes to the buffer. Only process complete lines (terminated by `\n`). Keep any trailing partial line in the buffer for the next chunk.
2. Use the `eventsource-stream` crate (which handles SSE parsing including cross-chunk buffering) as a processing layer rather than hand-rolling line parsing. Apply it to the stream for observation, then reconstruct the byte stream for forwarding.
3. If hand-rolling (to avoid the overhead of full SSE parsing when you only need the final usage chunk): scan the buffer for `\n`-delimited complete lines, extract and process them, retain the remainder.
4. Test with a mock server that deliberately fragments SSE events at awkward byte boundaries (mid-prefix, mid-JSON, mid-newline sequence).

**Phase to address:** First phase -- the buffering strategy is the foundational design decision. Everything else builds on correctly receiving complete SSE lines.

**Confidence:** HIGH -- the current code at handlers.rs:671-707 demonstrably uses per-chunk `text.lines()` with no cross-chunk buffer. TCP chunk splitting is well-documented networking behavior.

---

### Pitfall 2: Tee Architecture -- Forwarding Bytes While Also Parsing Them

**What goes wrong:** The current architecture passes the `reqwest` byte stream directly to `Body::from_stream(stream)` (handlers.rs:709), which means the bytes flow from reqwest to the axum response body. To intercept and extract usage, you need to both READ the bytes (to parse SSE events) and FORWARD the same bytes to the client -- a "tee" pattern. The naive approach has several failure modes:

- **Cloning every chunk:** Calling `.clone()` on each `Bytes` chunk doubles memory bandwidth. `bytes::Bytes` uses reference counting, so `.clone()` is cheap (just an Arc increment), but only if you don't then convert to `&str` or `String` for parsing -- that allocation IS expensive.
- **Buffering the entire stream:** Collecting all chunks to parse after completion defeats the purpose of streaming and causes memory to grow linearly with response size.
- **Channel-based tee:** Spawning a parsing task connected via `tokio::sync::mpsc` adds latency per chunk and complexity for error propagation.

The right approach for this codebase: keep the existing `.map()` closure pattern (handlers.rs:671-707) but add a cross-chunk buffer to it. The closure already has access to each chunk as it passes through. The `Bytes` does not need cloning because `std::str::from_utf8(bytes)` borrows the chunk in place. The parsed data (usage values) can be communicated back via a shared `Arc<Mutex<Option<Usage>>>` or a `tokio::sync::watch` channel.

**Why it happens:** "Tee" is conceptually simple but Rust's ownership model makes the naive implementation awkward. The stream is consumed by `Body::from_stream()`, and there's no built-in "observe while forwarding" combinator in `futures::StreamExt`.

**Consequences:**
- Over-engineering the tee pattern adds latency (channel overhead per chunk) or memory (full buffering)
- Under-engineering it loses data (no cross-chunk buffer) or breaks streaming (consuming the stream for parsing)
- Getting the ownership wrong causes the stream adapter closure to not be `Send + 'static`, preventing it from being used with `Body::from_stream()`

**Warning signs:**
- Calls to `Bytes::clone()` in the hot path for every chunk
- `tokio::spawn` for a parallel parsing task just to read side-channel data
- Stream adapter closure that captures non-Send types
- `String::from_utf8(bytes.to_vec())` instead of `std::str::from_utf8(&bytes)` (unnecessary allocation)

**How to avoid:**
1. Extend the existing `.map()` closure to maintain a `String` line buffer (captured by the closure). On each chunk, append to the buffer, extract complete lines, scan for usage. The `Bytes` pass through unmodified.
2. Store extracted usage in an `Arc<Mutex<Option<(u32, u32)>>>` shared between the stream closure and the completion handler. The closure writes; the completion handler (triggered when the stream ends) reads.
3. Use `std::str::from_utf8(&bytes)` to borrow the chunk without allocation. Only allocate when appending incomplete line fragments to the buffer.
4. The existing pattern at handlers.rs:671-707 already does almost exactly this -- it just lacks the cross-chunk buffer and the shared state for extracted values.

**Phase to address:** First phase -- the tee architecture determines the entire implementation shape.

**Confidence:** HIGH -- based on direct analysis of the current stream adapter pattern at handlers.rs:671-707 and Rust ownership constraints on `Body::from_stream()`.

---

### Pitfall 3: Database INSERT-then-UPDATE Race and the Fire-and-Forget Pattern

**What goes wrong:** The current code logs streaming requests BEFORE the stream is consumed (handlers.rs:166-203). The log entry is written with `input_tokens: None, output_tokens: None, cost_sats: None` because usage is unknown at that point. The new feature needs to UPDATE that row after the stream completes with the extracted usage data. This creates several problems:

1. **Race condition:** The fire-and-forget INSERT (`spawn_log_write` at handlers.rs:202) is a `tokio::spawn` that may not have completed by the time the stream starts. If the stream completes quickly (small response) or if the database is slow, the UPDATE may execute before the INSERT, finding zero rows to update.
2. **Stream outlives the handler:** The handler function returns the `Response` containing the stream body. The stream is consumed by axum's response writer AFTER the handler returns. The handler's scope (where the database pool and correlation_id live) is gone by the time the stream completes. Any on-complete callback needs to capture these values.
3. **Two-write pattern doubles database load:** Every streaming request now requires an INSERT + UPDATE instead of a single INSERT. For a local proxy this is fine, but the pattern should be intentional.

**Why it happens:** The current fire-and-forget INSERT was designed for a world where streaming requests have no usage data. Adding post-stream usage requires changing when and how the database write happens.

**Consequences:**
- UPDATE executes before INSERT: zero rows affected, usage data silently lost
- Captured values in stream closure cause lifetime issues or prevent the closure from being `Send + 'static`
- Database errors in the stream closure have no handler to report to (the handler already returned)

**Warning signs:**
- `UPDATE ... WHERE correlation_id = ?` affecting 0 rows in production logs
- Database pool cloned into the stream closure without testing the timing
- No test that verifies the INSERT-then-UPDATE sequence under concurrent load

**How to avoid:**

**Option A (recommended): Defer the INSERT until stream completion.**
Do not INSERT when the handler returns. Instead, let the stream's on-complete logic do a single INSERT with all data (including usage if available). This eliminates the race condition and the two-write pattern entirely. The tradeoff: if the stream is interrupted (client disconnect, server crash), no database record exists at all.

**Option B: INSERT placeholder, then UPDATE.**
Keep the current INSERT but `await` it (not fire-and-forget) to guarantee it completes before the stream starts. Then UPDATE after stream completion. This guarantees a record exists even for interrupted streams, but requires changing `spawn_log_write` to be awaitable for the streaming path.

**Option C: INSERT on stream completion, mark interrupted streams differently.**
Use the stream's `Drop` impl or a wrapper that detects whether the stream completed normally or was interrupted. INSERT on normal completion with full data. On interruption, INSERT with a special `error_message` indicating the stream was interrupted.

Option A is simplest and correct for a local proxy where incomplete records for crashed streams are not useful. Option B is better if you need records for interrupted streams for debugging.

**Phase to address:** Must be decided in the design phase before any implementation. The database write timing is an architectural decision.

**Confidence:** HIGH -- the race condition between `spawn_log_write` (fire-and-forget) and stream consumption is directly observable in the code at handlers.rs:166-203.

---

### Pitfall 4: Providers That Do Not Send Usage Data in Streams

**What goes wrong:** The OpenAI API only includes usage in streaming responses when the request contains `stream_options: {"include_usage": true}`. Without this parameter, the final chunk has no `usage` field. Many OpenAI-compatible providers have varying support:

- **OpenAI:** Requires `stream_options: {"include_usage": true}`. Without it, no usage in stream. With it, the final chunk before `data: [DONE]` has `choices: []` and `usage: {...}`.
- **Groq:** Puts usage in `x_groq.usage` field, NOT in the standard `usage` field. Does not support `stream_options`.
- **Anthropic (via OpenAI-compatible endpoints):** Uses a different SSE event structure entirely (`event: message_delta` with `usage` in the data). May or may not be normalized by Routstr providers.
- **OpenRouter:** Normalizes to OpenAI format but sends SSE comment lines (`:` prefix) as keep-alive heartbeats. Also sends errors mid-stream with `finish_reason: "error"`.
- **Ollama:** Has open issues about `stream_options` support. May or may not include usage.
- **Azure OpenAI:** Requires specific API version parameters for streaming usage support.

If arbstr injects `stream_options: {"include_usage": true}` into the forwarded request, it may break providers that do not support this parameter (they might return 400 Bad Request or ignore it).

**Why it happens:** "OpenAI-compatible" is a loose standard. Providers implement subsets of the API at different specification versions. The `stream_options` parameter was added to OpenAI in mid-2024 and adoption varies.

**Consequences:**
- Injecting `stream_options` breaks requests to providers that reject unknown parameters
- Not injecting it means no usage data from providers that require it
- Extracting from non-standard locations (`x_groq.usage`) requires provider-specific parsing
- Some providers will never send usage in streams, leaving permanent gaps in cost tracking

**Warning signs:**
- Testing against only one provider (especially OpenAI directly)
- Hard-coded `"usage"` field path for extraction
- No fallback for streams that complete without usage data
- No per-provider configuration for whether to inject `stream_options`

**How to avoid:**
1. **Do NOT inject `stream_options` by default.** Let the client decide. If the client sends `stream_options: {"include_usage": true}`, arbstr should pass it through and also extract the usage from the response. If the client does not send it, arbstr should not add it (this would be modifying the client's request semantics).
2. **Add an optional per-provider config flag** like `inject_stream_usage = true` that tells arbstr to add `stream_options` to requests for providers known to support it.
3. **Check multiple locations for usage data:** Standard `usage` field, `x_groq.usage`, and any `usage`-like fields in the SSE events. A flexible extraction function that tries multiple paths.
4. **Gracefully handle missing usage:** The database schema already supports NULL token fields. The code should log at `debug` level when a stream completes without usage, not `warn` or `error` -- this is expected behavior, not an error.
5. **Test with a mock provider that sends no usage, and verify the proxy handles it without error.**

**Phase to address:** Design phase -- the decision about whether to inject `stream_options` affects the request forwarding logic. The multi-provider extraction logic should be in a dedicated parsing module.

**Confidence:** HIGH -- OpenAI stream_options behavior verified via API documentation. Groq x_groq.usage verified via community reports and GitHub issues. OpenRouter SSE comments verified via OpenRouter documentation.

---

### Pitfall 5: Not Handling the `data: [DONE]` Sentinel Correctly

**What goes wrong:** OpenAI streams terminate with `data: [DONE]\n\n`. The current code checks for this (handlers.rs:679: `if data != "[DONE]"`). But the interception logic needs to know that `[DONE]` means the stream is COMPLETE and it is time to finalize usage extraction and trigger the database write. Several things go wrong:

1. **`[DONE]` may not arrive:** If the upstream connection drops, the stream ends without `[DONE]`. The stream's last chunk may be a partial SSE event or nothing. Usage extraction must handle both normal termination (`[DONE]` received) and abnormal termination (stream error or EOF without `[DONE]`).
2. **Usage chunk arrives BEFORE `[DONE]`:** The chunk containing `usage` data has `choices: []` and is the second-to-last SSE event. `[DONE]` is the last. If the code triggers finalization on `[DONE]`, it must have already processed the usage chunk. If processing is deferred to after `[DONE]`, the usage chunk might not be in the buffer.
3. **Some providers send `[DONE]` without usage:** If `stream_options` was not set, the last data chunk is the final content delta (with `finish_reason: "stop"`), followed immediately by `[DONE]`. There is no usage chunk. The finalization logic must not treat "no usage found" as an error.
4. **OpenRouter sends `[DONE]` but may also send mid-stream errors:** A stream can have `finish_reason: "error"` on a choice, followed by `[DONE]`. The finalization logic should detect the error and log accordingly.

**Why it happens:** `[DONE]` is a convention, not part of the SSE specification. It is OpenAI-specific and not all providers send it. Code that relies on `[DONE]` for finalization misses the stream-ended-without-DONE case.

**Consequences:**
- Usage data processed correctly when `[DONE]` arrives, but database write never triggers when the stream ends without `[DONE]`
- Memory leak: if finalization waits for `[DONE]` and it never comes, shared state (Arc<Mutex<...>>) lingers
- Incorrect error classification: a stream that completes normally without `[DONE]` is treated as an error

**Warning signs:**
- Finalization logic gated on `data == "[DONE]"` with no fallback for stream end
- No test for a stream that ends without `[DONE]` (just EOF)
- Shared state that is only cleaned up in the `[DONE]` handler

**How to avoid:**
1. Trigger finalization when the stream ENDS, not when `[DONE]` is received. In Rust's `Stream` trait, the stream signals completion by returning `Poll::Ready(None)`. Use a stream wrapper (or the `.map()` closure's drop) to detect stream end.
2. Use `[DONE]` as an optimization hint ("the next event is probably the end") but not as the sole trigger for finalization.
3. Implement a stream wrapper that implements `Drop` and triggers finalization. Or use `futures::StreamExt::chain` to append a finalization item.
4. The cleanest pattern: after the `.map()` closure extracts usage, append a `.chain(futures::stream::once(async { finalize() }))` that runs when the upstream stream is exhausted.

**Phase to address:** Implementation phase -- the finalization trigger is a tactical decision within the stream wrapper implementation.

**Confidence:** HIGH -- `[DONE]` handling visible at handlers.rs:679. Stream termination without `[DONE]` is a well-documented edge case in SSE.

---

### Pitfall 6: Stream Closure Lifetime and `Send + 'static` Constraints

**What goes wrong:** `Body::from_stream()` requires the stream to be `Send + 'static`. The `.map()` closure that processes each chunk must therefore only capture `Send + 'static` types. Adding state to the closure for cross-chunk buffering (a `String` buffer) and usage extraction (shared state back to the handler) introduces ownership constraints that are easy to violate:

- Capturing `&AppState` (which contains `Arc<Config>`, `Arc<ProviderRouter>`, `SqlitePool`) -- references are not `'static`
- Capturing `&str` slices (like `correlation_id`) -- references are not `'static`
- Capturing the `provider` reference -- not `'static`

The existing code (handlers.rs:669-707) avoids this by capturing only `move`d owned values. The `provider_name` is cloned before the closure. Adding more captured state (database pool, correlation_id, provider rates for cost calculation) requires the same discipline.

**Why it happens:** Rust's borrow checker enforces that closures passed to `Body::from_stream()` cannot borrow from the handler's stack frame because the handler returns before the stream is consumed. Every value the closure needs must be owned (moved or cloned into the closure).

**Consequences:**
- Compilation errors that are confusing: "closure may outlive the current function" or "`T` is not `Send`"
- Developers work around it by cloning large structures (entire `AppState`) into the closure, which is wasteful
- `Mutex` in the closure makes it not `Send` if the `Mutex` is `std::sync::Mutex` but the code tries to hold it across an `.await` (actually, `std::sync::Mutex` IS `Send` -- it is `tokio::sync::Mutex` that has different properties. But `std::sync::Mutex` must not be held across await points)

**Warning signs:**
- `Send + 'static` compilation errors when modifying the stream closure
- Cloning entire `AppState` or `Config` into the closure
- Using `Rc<RefCell<...>>` instead of `Arc<Mutex<...>>` (Rc is not Send)
- Holding a `MutexGuard` across the `chunk.map_err()` call

**How to avoid:**
1. Clone only what you need into the closure: `pool.clone()` (SqlitePool is Arc-based and cheap to clone), `correlation_id.clone()` (String), provider rate info (copy of u64 values), `provider_name.clone()`.
2. For shared mutable state (the extracted usage), use `Arc<Mutex<Option<(u32, u32)>>>`. Create it before the closure, clone the Arc into the closure, and keep a handle outside.
3. Do not hold any MutexGuard across `.await` points (there are none in the current `.map()` closure since it is synchronous, but be careful if refactoring to `.then()` or async closures).
4. Capture a small struct of needed values rather than individual clones to keep the closure signature clean.

**Phase to address:** Implementation phase -- this is mechanical Rust ownership wrangling, but must be anticipated in the design.

**Confidence:** HIGH -- Rust's Send + 'static requirements are compiler-enforced. The current closure at handlers.rs:670-707 demonstrates the pattern.

---

## Moderate Pitfalls

Mistakes that cause subtle data loss, incorrect metrics, or performance issues.

---

### Pitfall 7: SSE Comment Lines and Keep-Alive Events

**What goes wrong:** The SSE specification defines comment lines starting with `:` (colon). These are used by providers as keep-alive heartbeats to prevent proxies and load balancers from timing out idle connections. OpenRouter explicitly documents sending SSE comments during processing. The current parsing code (handlers.rs:678) only looks for `data: ` prefixed lines and would ignore comments -- which is correct for data extraction. However, there are edge cases:

1. **Comment lines in the buffer:** If using a line buffer for cross-chunk parsing, comment lines accumulate in the buffer and must be recognized and skipped (or they interfere with line parsing).
2. **Event type lines:** The SSE spec allows `event: ` lines. Anthropic's API uses named events (`event: message_start`, `event: content_block_delta`, `event: message_delta`). If a Routstr provider passes through Anthropic events without normalization, the `data:` line follows an `event:` line, and the usage data may be on a `data:` line that is part of a `message_delta` event, not a standalone data event.
3. **Retry lines:** SSE allows `retry:` lines that suggest reconnection intervals. These should be ignored by the parser but forwarded to the client.
4. **Empty lines as event delimiters:** In the SSE spec, a blank line (`\n\n`) delimits events. Multiple `data:` lines before a blank line are concatenated into a single event data. If a provider sends multi-line data (unlikely for JSON but allowed by spec), the parser must concatenate rather than treating each `data:` line as an independent event.

**Why it happens:** Most LLM providers use a simplified SSE format (single `data:` line per event), so developers never encounter multi-line data or event types. Then a non-standard provider breaks the parser.

**How to avoid:**
1. For usage extraction, only process lines matching `^data: `. Skip lines matching `^:`, `^event:`, `^retry:`, and blank lines. This is what the current code does and it is correct for the simplified LLM SSE format.
2. Do not attempt to implement a full SSE parser unless you need to handle Anthropic-style named events. For arbstr's use case (extract a JSON object from the last meaningful data line), simple prefix matching is sufficient.
3. Ensure the line buffer correctly handles blank lines as event delimiters without treating them as data.
4. Forward ALL bytes to the client unmodified -- the parsing is observation-only. Never filter or modify the stream.

**Phase to address:** Implementation phase -- the line parser design should account for these non-data line types.

**Confidence:** HIGH for OpenRouter comments (documented). MEDIUM for Anthropic-style events through Routstr providers (depends on provider normalization).

---

### Pitfall 8: Memory Growth on Long-Running Streams

**What goes wrong:** If the line buffer for cross-chunk parsing is never cleared, it grows with every chunk. For a typical chat completion (a few hundred tokens), this is negligible. But for very long responses (code generation, long documents), the buffer could accumulate megabytes. Worse, if the buffer retains all processed data (not just the incomplete trailing line), memory grows linearly with response size.

Additionally, the shared state for extracted usage (`Arc<Mutex<Option<Usage>>>`) is small and bounded. But if the implementation accumulates ALL parsed events (for debugging or fallback counting), that state grows unboundedly.

**Why it happens:** The buffer pattern for cross-chunk SSE parsing naturally accumulates data. Without explicit truncation after processing complete lines, the buffer retains processed data.

**How to avoid:**
1. After extracting complete lines from the buffer, drain the processed portion. Only the trailing incomplete line fragment remains in the buffer. `buffer.drain(..last_newline_pos)` or equivalent.
2. For the stream closure, process each complete line immediately and discard it. Only retain the pending partial line and the last-seen usage values.
3. Set a maximum buffer size (e.g., 64KB). If a single SSE line exceeds this, something is wrong. Log a warning and skip that line rather than OOM.
4. Do not accumulate parsed events in a `Vec`. Extract the usage values and discard everything else.

**Warning signs:**
- `String::push_str()` without corresponding `drain()` or `truncate()`
- `Vec<ParsedEvent>` growing for the duration of the stream
- Memory usage correlated with response length

**Phase to address:** Implementation phase -- buffer management is part of the line parser.

**Confidence:** HIGH -- standard concern for any stream processing with buffering.

---

### Pitfall 9: Latency Impact of Per-Chunk JSON Parsing

**What goes wrong:** Parsing every SSE data line as JSON to check for `usage` adds CPU overhead to every chunk in the stream. For a typical streaming response with hundreds of chunks, this means hundreds of `serde_json::from_str` calls. The current code already does this (handlers.rs:680-696), so the baseline overhead exists. But adding a line buffer, string operations, and more complex extraction could measurably increase per-chunk latency.

The key insight: usage data ONLY appears in the final chunk (or second-to-last, before `[DONE]`). Parsing every intermediate chunk for `usage` is wasted work.

**Why it happens:** It is simpler to parse every chunk uniformly than to detect which chunk is the final one. The "final chunk" is only identifiable after the fact (when the stream ends or `[DONE]` arrives).

**How to avoid:**
1. **Optimization: only parse chunks that might contain usage.** Usage chunks have `"usage"` as a substring. Do a cheap `bytes.contains("usage")` or `line.contains("\"usage\"")` check before parsing JSON. This skips JSON parsing for 99%+ of chunks.
2. **Optimization: only parse the last few data lines.** After `[DONE]` or stream end, parse the last buffered data line for usage. During the stream, just buffer line boundaries without parsing.
3. **Measurement: benchmark the overhead.** Time the stream closure with and without parsing. For a local proxy on localhost, per-chunk overhead under 10 microseconds is irrelevant. Over a network, the network latency dwarfs parsing time.
4. The current code already parses every chunk -- so any optimization is an improvement, not a regression.

**Warning signs:**
- `serde_json::from_str` called for every chunk in profiling flamegraphs
- Measurable TTFB (time to first byte) increase after adding interception
- `serde_json::Value` allocations dominating memory profiles during streaming

**Phase to address:** Second phase (optimization). Get correctness first, then optimize if benchmarks show impact.

**Confidence:** MEDIUM -- the overhead exists but may be negligible for a local proxy. Optimize only if measured.

---

### Pitfall 10: `ChatCompletionRequest` Does Not Forward Unknown Fields

**What goes wrong:** The `ChatCompletionRequest` struct (types.rs:7-26) uses explicit field definitions with `#[serde(skip_serializing_if = "Option::is_none")]`. It does NOT have `#[serde(flatten)] extra: HashMap<String, Value>` or similar catch-all. This means if a client sends `stream_options: {"include_usage": true}` in their request, it is silently dropped during deserialization. The upstream provider never receives it.

This is a pre-existing bug, not introduced by the streaming feature. But it becomes critical now because `stream_options` is the mechanism by which clients opt into streaming usage data from OpenAI-compatible providers.

**Why it happens:** The request struct was designed to model the known OpenAI fields. `stream_options` was added to the OpenAI API in mid-2024 and was not included in the struct definition.

**Consequences:**
- Clients that explicitly request streaming usage get no usage data because the parameter is stripped
- arbstr silently modifies the client's request semantics
- If arbstr wants to inject `stream_options` for its own purposes, it has no field to set

**Warning signs:**
- Client sends `stream_options` but provider never receives it
- New OpenAI API parameters are silently dropped
- Testing against arbstr with `stream_options` shows no usage, but direct provider calls do

**How to avoid:**
1. **Add `stream_options` to `ChatCompletionRequest`:**
   ```rust
   #[serde(skip_serializing_if = "Option::is_none")]
   pub stream_options: Option<StreamOptions>,
   ```
   Where `StreamOptions` contains `include_usage: Option<bool>`.
2. **Alternatively, use a catch-all:** Add `#[serde(flatten)] pub extra: HashMap<String, serde_json::Value>` to forward all unknown fields. This is more future-proof but means you cannot inspect `stream_options` by type.
3. **Best approach for arbstr:** Add the typed `stream_options` field AND a `#[serde(flatten)] extra` catch-all. This gives you typed access to `stream_options` while preserving all other unknown fields.

**Phase to address:** First phase -- this must be fixed before stream usage extraction can work, because without it, the client cannot request streaming usage from the provider through arbstr.

**Confidence:** HIGH -- directly observable in types.rs:7-26 that `stream_options` is not a field.

---

### Pitfall 11: Counting Tokens by Summing Deltas vs. Using Final Usage Object

**What goes wrong:** There are two approaches to getting token counts from a stream:
1. **Count deltas:** Sum the content lengths from each `delta.content` chunk to estimate output tokens. Estimate input tokens from the request.
2. **Use the final usage object:** Extract `prompt_tokens` and `completion_tokens` from the usage chunk that the provider sends.

Approach 1 (counting deltas) is unreliable because tokens are not characters. A tokenizer maps variable-length character sequences to tokens, and the delta content strings do not correspond to token boundaries. You would need a tokenizer (like tiktoken) running in the proxy to count tokens from text, and different models use different tokenizers.

Approach 2 (final usage object) is authoritative but may not be available (see Pitfall 4).

The mistake is using approach 1 as a "fallback" when approach 2 is unavailable, which produces inaccurate counts that feed into cost calculations.

**Why it happens:** It feels wasteful to have a stream of text pass through without counting it. The "approximate by counting characters/words" approach seems reasonable until you realize it can be off by 30-50% due to tokenizer differences.

**How to avoid:**
1. Use ONLY the provider-reported usage object for token counts. If the provider does not send usage, record NULL tokens (not estimated tokens).
2. Do NOT implement client-side token counting as a fallback. It gives a false sense of accuracy and the error margins make cost calculations unreliable.
3. The database schema already supports NULL token fields. The cost tracking already handles NULL gracefully. There is no need for estimated counts.
4. If approximate counting is needed in the future (e.g., for budget enforcement before stream completes), make it a separate field (`estimated_output_tokens`) distinct from the authoritative `output_tokens`.

**Phase to address:** Design phase -- decide that token counting is provider-authoritative-only and do not build estimation infrastructure.

**Confidence:** HIGH -- tokenizer mismatch between client-side counting and provider-side counting is well-documented.

---

## Technical Debt Patterns

Shortcuts that seem reasonable but create long-term problems.

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Parse every chunk as JSON | Simple uniform logic | CPU overhead on every chunk, thousands of unnecessary allocations per stream | MVP -- optimize later if profiling shows impact |
| Clone entire `AppState` into stream closure | Compiles easily | Captures database pool, config, router -- more than needed. Conceptually unclear what the closure uses | Never -- clone only the 4-5 values actually needed |
| `unwrap()` on `from_utf8` in stream closure | Avoids error handling | Non-UTF8 bytes from a misbehaving provider crash the proxy | Never -- use `from_utf8` and skip non-UTF8 chunks |
| Hardcode `"usage"` field path | Works for OpenAI | Breaks for Groq (`x_groq.usage`) and future providers | MVP if only targeting OpenAI-compatible providers initially. Add provider-specific extraction later |
| Fire-and-forget UPDATE for post-stream usage | Simple, no await in closure | Silent data loss if UPDATE fails, no retry, no logging of failure | Acceptable for local proxy -- log the failure at warn level |
| Skip stream interception for non-200 responses | Avoids parsing error streams | Misses error metadata that could inform routing decisions | Acceptable -- error responses are already logged before streaming starts |

## Integration Gotchas

Common mistakes when connecting to upstream LLM providers via SSE.

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| OpenAI streaming | Not sending `stream_options: {"include_usage": true}` | Either pass through client's `stream_options` or configure per-provider injection |
| Groq streaming | Looking for `usage` in standard location | Check both `usage` and `x_groq.usage` in the final chunk |
| OpenRouter streaming | Not handling SSE comment lines (`:` prefix) | Forward all bytes to client; skip comment lines in parser |
| OpenRouter mid-stream errors | Treating `finish_reason: "error"` as normal completion | Detect error finish reason, log stream as failed |
| Azure OpenAI | Assuming `stream_options` is supported on all API versions | Check API version; older versions do not support it |
| Provider timeout (no `[DONE]`) | Waiting for `[DONE]` to finalize | Finalize on stream end (None from poll), not on `[DONE]` receipt |
| HTTP chunked encoding | Assuming chunk = SSE event | Buffer across chunks, parse complete lines only |

## Performance Traps

Patterns that work at small scale but fail as usage grows.

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| JSON-parsing every SSE chunk | High CPU usage during streaming, increased latency | Substring check for `"usage"` before parsing | 100+ concurrent streams |
| Unbounded line buffer | Memory grows with response length | Drain processed lines, cap buffer at 64KB | Very long responses (10K+ tokens) |
| Synchronous Mutex in stream closure | Contention under concurrent streams (unlikely with per-stream mutex) | Use per-stream state, not shared global state | Not a real risk if each stream has its own Mutex |
| `String::from_utf8(bytes.to_vec())` per chunk | Double allocation (vec + string) per chunk | `std::str::from_utf8(&bytes)` borrows in place | High throughput, many concurrent streams |
| SQLite UPDATE per completed stream | WAL contention under concurrent completions | Batch updates via channel, or accept per-request latency | 50+ concurrent streams completing simultaneously |

## "Looks Done But Isn't" Checklist

Things that appear complete but are missing critical pieces.

- [ ] **SSE parsing:** Often missing cross-chunk buffering -- verify with a test that splits `data: {"usage":...}\n` across two chunks at every possible byte position
- [ ] **Usage extraction:** Often missing Groq `x_groq.usage` path -- verify with a mock Groq response
- [ ] **Stream finalization:** Often missing the "stream ended without [DONE]" case -- verify with a mock that sends EOF after the last data chunk without `[DONE]`
- [ ] **Request forwarding:** Often drops `stream_options` -- verify by capturing the upstream request and checking it contains `stream_options`
- [ ] **Database timing:** Often has INSERT/UPDATE race -- verify with a test that delays the INSERT and asserts the UPDATE still succeeds (or that the single-write approach works)
- [ ] **Client disconnect:** Often leaks the parsing state -- verify that when a client disconnects mid-stream, the stream closure's state is dropped and any finalization still runs
- [ ] **Non-UTF8 resilience:** Often panics on non-UTF8 bytes -- verify with a mock that sends 0xFF bytes mid-stream
- [ ] **Error streams:** Often tries to parse error response bodies as SSE -- verify that non-200 upstream responses are handled before reaching the SSE parser
- [ ] **Empty streams:** Often fails on a stream that immediately sends `[DONE]` with no content chunks -- verify the zero-content-chunk case

## Recovery Strategies

When pitfalls occur despite prevention, how to recover.

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Split-chunk parsing bug (Pitfall 1) | LOW | Add buffering layer, existing data is not corrupted (just NULL usage fields in DB) |
| Database INSERT/UPDATE race (Pitfall 3) | LOW | Switch to single-INSERT-on-completion, backfill NULL usage from provider logs if needed |
| Missing `stream_options` forwarding (Pitfall 10) | LOW | Add field to request struct, redeploy. No data migration needed |
| Provider-specific format not handled (Pitfall 4) | LOW | Add extraction path for new provider format, existing records stay with NULL usage |
| Memory leak from unbounded buffer (Pitfall 8) | MEDIUM | Add buffer drain, may require restart under memory pressure |
| Wrong token counts from delta counting (Pitfall 11) | HIGH | Must discard all estimated counts, cannot retroactively correct cost calculations |

## Pitfall-to-Phase Mapping

How roadmap phases should address these pitfalls.

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| Pitfall 1: Chunk splitting | Phase 1 (Core SSE parser) | Unit test: SSE events split at every byte position |
| Pitfall 2: Tee architecture | Phase 1 (Stream wrapper design) | Integration test: client receives identical bytes as direct provider call |
| Pitfall 3: DB write timing | Phase 1 (Design decision) | Integration test: completed stream has non-NULL usage in DB |
| Pitfall 4: Provider format differences | Phase 1 (Design) + Phase 2 (Provider-specific) | Mock tests per provider format |
| Pitfall 5: [DONE] handling | Phase 1 (Finalization logic) | Test: stream with [DONE], stream without [DONE], stream with only [DONE] |
| Pitfall 6: Send + 'static | Phase 1 (Implementation) | Compiler enforces this -- if it compiles, it works |
| Pitfall 7: SSE comment lines | Phase 1 (Parser) | Test: stream with `: keep-alive` comment lines interspersed |
| Pitfall 8: Memory growth | Phase 1 (Buffer management) | Test: stream 1MB of SSE data, assert memory does not grow linearly |
| Pitfall 9: Parsing overhead | Phase 2 (Optimization) | Benchmark: TTFB and throughput with and without interception |
| Pitfall 10: stream_options forwarding | Phase 1 (Request struct) | Test: client sends stream_options, upstream receives it |
| Pitfall 11: Token counting approach | Phase 1 (Design decision) | Code review: no delta-counting code exists |

## Sources

- Direct codebase analysis of arbstr `src/proxy/handlers.rs` (streaming handler at lines 665-729), `src/proxy/types.rs` (request struct), `src/storage/logging.rs` (fire-and-forget pattern)
- [OpenAI Streaming API Reference](https://platform.openai.com/docs/api-reference/chat-streaming) -- stream_options, include_usage, final chunk format
- [OpenAI Developers announcement](https://x.com/OpenAIDevs/status/1787573348496773423) -- stream_options feature launch
- [OpenRouter Streaming Documentation](https://openrouter.ai/docs/api/reference/streaming) -- SSE comments, mid-stream errors, normalization
- [Groq streaming usage discussion](https://github.com/vercel/ai/discussions/2290) -- x_groq.usage non-standard field
- [LiteLLM streaming token usage issue](https://github.com/BerriAI/litellm/issues/3553) -- provider-specific usage format differences
- [Axum SSE backpressure discussion](https://users.rust-lang.org/t/axum-sse-and-backpressure/133061) -- backpressure handling patterns
- [Axum proxy streaming discussion](https://github.com/tokio-rs/axum/discussions/1821) -- bytes_stream proxying patterns
- [Adam Chalmers: Static streams for faster async proxies](https://blog.adamchalmers.com/streaming-proxy/) -- stream forwarding patterns in Rust
- [eventsource-stream crate](https://docs.rs/eventsource-stream/) -- Rust SSE parsing with cross-chunk buffering
- [Axum client disconnect detection](https://users.rust-lang.org/t/how-to-detect-an-a-dropped-sse-server-sent-event-client-using-axum/101028) -- Drop-based cleanup patterns
- [Simon Willison: How streaming LLM APIs work](https://til.simonwillison.net/llms/streaming-llm-apis) -- SSE format comparison across providers
- [Anthropic Streaming Messages](https://platform.claude.com/docs/en/build-with-claude/streaming) -- event-typed SSE format differences
- [Tokio/Reqwest byte stream to lines](https://users.rust-lang.org/t/tokio-reqwest-byte-stream-to-lines/65258) -- line buffering for reqwest byte streams

---
*Pitfalls research for: SSE stream interception and token extraction in Rust proxy*
*Researched: 2026-02-15*
