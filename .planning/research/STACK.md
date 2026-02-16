# Technology Stack: SSE Stream Parsing for Streaming Observability

**Project:** arbstr v1.2 - Streaming token extraction and cost tracking
**Researched:** 2026-02-15
**Overall confidence:** HIGH

## Scope

This research covers ONLY the stack additions/changes needed for:
1. Parsing SSE `data:` lines from streaming OpenAI-compatible responses
2. Intercepting the stream to extract token usage without buffering the full response
3. Updating SQLite log entries after stream completion with extracted token counts
4. Injecting `stream_options: {"include_usage": true}` into forwarded requests

Everything else in the existing stack is unchanged and validated from v1/v1.1.

## Existing Stack (No Changes Needed)

These dependencies remain correct and require no modifications:

| Technology | Version (locked) | Purpose | Status |
|------------|-----------------|---------|--------|
| tokio | 1.x (full) | Async runtime, channels, spawn | Keep as-is |
| axum | 0.7 | HTTP server, Body::from_stream | Keep as-is |
| reqwest | 0.12 (stream feature) | HTTP client, bytes_stream() | Keep as-is |
| serde / serde_json | 1.x | JSON parsing of SSE data payloads | Keep as-is |
| sqlx | 0.8 (sqlite, runtime-tokio, migrate) | Log updates after stream completion | Keep as-is |
| futures | 0.3.31 | StreamExt for stream transformation | Keep as-is |
| tracing | 0.1 | Debug logging of extracted tokens | Keep as-is |
| bytes | 1.11.0 (transitive) | Already in dep tree via axum/reqwest | Keep as-is |
| pin-project-lite | 0.2.16 (transitive) | Already in dep tree via futures-util | Keep as-is |

**Critical existing capabilities already in the dependency tree:**
- `reqwest` with `stream` feature provides `bytes_stream()` returning `impl Stream<Item = Result<Bytes, reqwest::Error>>`
- `futures::StreamExt` provides `.map()` for stream transformation (already used in `handle_streaming_response`)
- `axum::body::Body::from_stream()` accepts any `Stream<Item = Result<impl Into<Bytes>, impl Into<BoxError>>>` (already used)
- `tokio::sync::oneshot` and `tokio::sync::watch` are available via `tokio = { features = ["full"] }`
- `serde_json::from_str::<serde_json::Value>()` for parsing SSE data payloads (already used in the current stream map closure)

## New Dependencies Required

### None.

**The existing dependency set is sufficient for streaming SSE token extraction.** No new crates are needed. Here is why:

### Why NOT eventsource-stream

| Crate | Version | Downloads/mo | Why Not |
|-------|---------|-------------|---------|
| `eventsource-stream` | 0.2.3 | ~271K | Provides `Eventsource` trait that wraps `bytes_stream()` into typed `Event` structs. Overkill -- arbstr only needs to extract `data:` line payloads, not implement full SSE spec (event types, IDs, retry). The existing code already does `strip_prefix("data: ")` and it works. Adding this crate gains nothing meaningful for the use case. |
| `reqwest-eventsource` | 0.6.x | ~120K | High-level EventSource client with auto-reconnect. Completely wrong for a proxy -- arbstr is forwarding the stream, not consuming it as a client. It replaces the reqwest request builder, which conflicts with arbstr's existing request construction (custom headers, Idempotency-Key, etc.). |
| `async-sse` | 5.x | Low | Surf-ecosystem crate. Wrong runtime (async-std). Not compatible with tokio/axum. |

### Why NOT async-stream

| Crate | Version | Why Not |
|-------|---------|---------|
| `async-stream` | 0.3.6 | Provides `stream!` macro for generator-style streams. Convenient but unnecessary -- the existing `bytes_stream().map()` pattern with `futures::StreamExt` is already sufficient and already used in the codebase. Adding `async-stream` would introduce a second stream construction pattern for no benefit. |

### Why NOT tokio-stream (as direct dependency)

`tokio-stream` 0.1.18 is already in the transitive dependency tree (via sqlx). However, adding it as a direct dependency is unnecessary because `futures::StreamExt` (already a direct dependency) provides all the needed combinators: `.map()`, `.then()`, and the ability to construct streams from iterators. The `futures` crate is the canonical stream toolkit for this codebase.

## Implementation Stack: What Existing Tools Provide

### 1. SSE Line Parsing: Hand-rolled (Already Exists)

The current `handle_streaming_response` in `handlers.rs` already has SSE parsing logic:

```rust
// Current code (lines 671-707) -- already parses SSE data lines
if let Ok(text) = std::str::from_utf8(bytes) {
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            if data != "[DONE]" {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                    // extract usage...
                }
            }
        }
    }
}
```

**What changes:** This logic moves from a debug-only trace into an actual capture mechanism that writes extracted usage to shared state. The parsing approach itself does not change. The existing `strip_prefix("data: ")` + `serde_json::from_str` pattern is correct for OpenAI-compatible SSE.

**Why hand-rolled is correct here:**
- OpenAI SSE format is trivial: `data: {json}\n\n` or `data: [DONE]\n\n`
- No event types, no IDs, no retry fields to parse
- A library would add complexity (Event struct unpacking) for zero benefit
- The prior phase-02 research explicitly recommended this: "arbstr only needs to read `data:` lines, not implement full SSE spec"

### 2. Stream Transformation: `futures::StreamExt::map()` (Already Used)

The stream transformation pattern uses `.map()` on the bytes stream to inspect each chunk while passing it through unmodified:

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

let captured_usage: Arc<Mutex<Option<(u32, u32)>>> = Arc::new(Mutex::new(None));
let usage_clone = captured_usage.clone();

let stream = upstream_response.bytes_stream().map(move |chunk| {
    if let Ok(ref bytes) = chunk {
        // Parse SSE lines, extract usage, store in captured_usage
    }
    chunk.map_err(std::io::Error::other)
});

let body = Body::from_stream(stream);
```

**This pattern already exists in the codebase** (handlers.rs line 671). The change is making the captured usage accessible after stream completion for the log update.

### 3. Post-Stream Notification: `tokio::sync::oneshot` (Already Available)

To update the database log after the stream completes, use a `tokio::sync::oneshot` channel. The stream wrapper sends the captured usage when the stream ends (on `[DONE]` or drop). A spawned task receives it and issues the SQL UPDATE.

```rust
use tokio::sync::oneshot;

let (usage_tx, usage_rx) = oneshot::channel::<Option<(u32, u32)>>();

// Stream wrapper: on [DONE] or drop, send captured usage
// Spawned task: await usage_rx, then UPDATE requests SET input_tokens=?, output_tokens=? WHERE correlation_id=?
```

**Why oneshot:** Already in `tokio` with `full` features. Single value, single consumer. No new dependency. The alternative (`tokio::sync::watch`) is heavier than needed for a single notification.

### 4. Log Update: `sqlx::query()` UPDATE (Already Available)

The database layer already supports fire-and-forget writes via `spawn_log_write`. The post-stream update uses the same pattern but with an UPDATE instead of INSERT:

```rust
sqlx::query(
    "UPDATE requests SET input_tokens = ?, output_tokens = ?, cost_sats = ?
     WHERE correlation_id = ?"
)
```

**No schema changes needed.** The `requests` table already has nullable `input_tokens`, `output_tokens`, and `cost_sats` columns. The initial INSERT logs them as NULL; the post-stream UPDATE fills them in.

### 5. Request Mutation for `stream_options`: `serde_json::Value` (Already Available)

To inject `stream_options: {"include_usage": true}` into the forwarded request, serialize the `ChatCompletionRequest` to `serde_json::Value`, insert the field, and send the modified JSON:

```rust
let mut body = serde_json::to_value(&request)?;
if request.stream == Some(true) {
    body.as_object_mut().unwrap().insert(
        "stream_options".to_string(),
        serde_json::json!({"include_usage": true}),
    );
}
```

**Why this approach:** The `ChatCompletionRequest` type does not need a `stream_options` field because arbstr always injects it for streaming requests. This keeps the type clean and avoids exposing an implementation detail to callers.

### 6. Chunk Boundary Buffering: `String` line buffer (std only)

The critical pitfall from prior research: TCP chunks don't align with SSE line boundaries. A `data: {...}` line could be split across two `bytes_stream()` chunks. The fix is a simple line buffer:

```rust
let mut line_buffer = String::new();

// In the map closure:
line_buffer.push_str(text);
while let Some(newline_pos) = line_buffer.find('\n') {
    let line = &line_buffer[..newline_pos];
    // Process complete line
    line_buffer = line_buffer[newline_pos + 1..].to_string();
}
```

**No crate needed.** This is ~10 lines of code. The `eventsource-stream` crate internally does the same thing but wrapped in a `Stream` adapter -- unnecessary overhead for our single use case.

**Note on closure state:** The line buffer must live inside the `.map()` closure as mutable captured state. Since `.map()` takes `FnMut`, mutable captures work. The buffer is `String`, which is `Send`, satisfying `Body::from_stream()` requirements.

## OpenAI Streaming Format Reference

This is the protocol arbstr must parse. Verified against OpenAI documentation.

### Normal Chunks (content delivery)

```
data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":null}

```

### Final Usage Chunk (when `stream_options.include_usage = true`)

```
data: {"id":"chatcmpl-123","object":"chat.completion.chunk","created":1694268190,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":12,"total_tokens":21}}

data: [DONE]

```

**Key facts:**
- Usage chunk has `"choices": []` (empty array)
- Usage chunk comes immediately before `data: [DONE]`
- All other chunks have `"usage": null`
- Without `stream_options`, no chunk contains usage data
- The `\n\n` (double newline) separates SSE events

### Provider Compatibility

| Provider | `stream_options` support | Confidence |
|----------|------------------------|------------|
| OpenAI | YES -- documented, official | HIGH |
| vLLM | YES -- documented as `stream_include_usage` (flattened) and standard format | MEDIUM |
| Ollama | UNCLEAR -- OpenAI compatibility mode may not support it | LOW |
| Routstr (marketplace) | DEPENDS on underlying provider | LOW |

**Mitigation for unsupported providers:** If the provider ignores `stream_options`, no usage chunk will appear. The stream will end with `data: [DONE]` and no usage is captured. The fallback is logging NULL tokens/cost -- identical to current behavior. This is safe degradation, not an error.

## ChatCompletionRequest Type Changes

The `stream_options` field should NOT be added to the `ChatCompletionRequest` struct. Instead, arbstr injects it at the serialization layer when forwarding to the provider. This keeps the public API type clean and makes it clear that `stream_options` is an arbstr implementation detail, not a client-facing parameter.

If the client sends `stream_options` in their request, it will be preserved in `serde_json::Value` passthrough (if we switch to Value-based forwarding) or ignored (if we keep typed deserialization). Either behavior is acceptable -- arbstr controls what it needs.

## ChatCompletionChunk Type Enhancement

The existing `ChatCompletionChunk` struct in `types.rs` lacks a `usage` field. Add it:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,  // NEW: present in final chunk when include_usage=true
}
```

**However:** Using typed deserialization (`serde_json::from_str::<ChatCompletionChunk>()`) is heavier than needed for usage extraction. The current `serde_json::Value` approach is better because:
1. It handles unknown fields gracefully (providers may add fields)
2. It's a single allocation for the whole chunk
3. We only need to read `usage.prompt_tokens` and `usage.completion_tokens`

**Recommendation:** Keep using `serde_json::Value` for SSE data parsing. Update the `ChatCompletionChunk` type for documentation completeness but don't rely on it for parsing.

## Cargo.toml Changes Summary

```toml
# NO CHANGES to [dependencies]
# Everything needed is already present:
# - futures = "0.3"           (StreamExt for stream transformation)
# - tokio = { features = ["full"] }  (oneshot channel for post-stream notification)
# - serde_json = "1"          (SSE data payload parsing)
# - sqlx = { features = ["sqlite"] } (UPDATE query for log amendment)
# - reqwest = { features = ["stream"] } (bytes_stream() for SSE chunks)
```

**Net dependency change: 0.** Zero new crates. This milestone uses only existing dependencies.

## What NOT to Add

| Technology | Why Not |
|------------|---------|
| `eventsource-stream` | SSE parsing library. Overkill for `strip_prefix("data: ")` on a known-simple format. Adds a dependency for ~10 lines of hand-rolled code. The OpenAI SSE format uses only `data:` lines -- no event types, IDs, or retry semantics to parse. |
| `reqwest-eventsource` | EventSource client with reconnection. Wrong abstraction -- arbstr is a proxy forwarding streams, not an SSE consumer. Conflicts with existing request construction. |
| `async-stream` | Stream generator macro. The existing `bytes_stream().map()` pattern is sufficient and already used. Adding a second stream construction approach would be confusing. |
| `tokio-stream` (direct dep) | Already transitive. `futures::StreamExt` covers all needed combinators. Adding it directly would create two competing `StreamExt` imports. |
| `sse-codec` / `sse-stream` | Various small SSE crates. Low adoption, unmaintained, solve a problem we don't have. |
| `bytes` (direct dep) | Already transitive via axum/reqwest. The stream transformation uses `bytes::Bytes` via reqwest but doesn't need direct import -- the type is re-exported through reqwest. |
| `pin-project` / `pin-project-lite` (direct dep) | Already transitive. Only needed if implementing custom `Stream` types, which we don't need -- `StreamExt::map()` handles our case. |

## Integration Points with Existing Code

### Files That Change

| File | Change | Why |
|------|--------|-----|
| `src/proxy/handlers.rs` | Major refactor of `handle_streaming_response` | Add line buffering, usage capture, post-stream notification |
| `src/proxy/handlers.rs` | Modify `send_to_provider` | Inject `stream_options` into request body for streaming |
| `src/proxy/handlers.rs` | Modify streaming path in `chat_completions` | Wire up oneshot channel, spawn post-stream log update task |
| `src/storage/logging.rs` | Add `update_streaming_usage` function | SQL UPDATE for post-stream token/cost fill-in |
| `src/proxy/types.rs` | Add `usage` field to `ChatCompletionChunk` | Documentation completeness (not used for parsing) |

### Files That Don't Change

| File | Why Not |
|------|---------|
| `Cargo.toml` | No new dependencies |
| `src/config.rs` | Config unchanged |
| `src/router/` | Routing logic unchanged |
| `src/error.rs` | No new error variants needed (stream parsing failures are logged as warnings, not errors) |
| `migrations/` | No schema changes (columns already nullable) |

## Version Verification

| Crate | Version (locked) | Verified Via | Confidence |
|-------|-----------------|-------------|------------|
| futures | 0.3.31 | Cargo.lock inspection | HIGH |
| tokio | 1.x (full features) | Cargo.toml + Cargo.lock | HIGH |
| reqwest | 0.12 (stream feature) | Cargo.toml | HIGH |
| serde_json | 1.x | Cargo.toml | HIGH |
| sqlx | 0.8 (sqlite, migrate) | Cargo.toml | HIGH |
| bytes | 1.11.0 (transitive) | Cargo.lock | HIGH |

## Sources

### Primary (HIGH confidence)
- Local codebase analysis -- all handler, types, storage, and retry source files read and verified
- [Cargo.lock inspection](Cargo.lock) -- confirmed futures 0.3.31, bytes 1.11.0, tokio-stream 0.1.18 as transitive deps
- Prior phase-02 research (`.planning/phases/02-request-logging/02-RESEARCH.md`) -- SSE parsing patterns, chunk boundary pitfall, `strip_prefix` recommendation
- [OpenAI Chat Streaming API reference](https://platform.openai.com/docs/api-reference/chat-streaming) -- `stream_options`, `include_usage`, final chunk format
- [OpenAI streaming usage stats announcement](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156) -- `stream_options.include_usage`, extra chunk behavior

### Secondary (MEDIUM confidence)
- [eventsource-stream on lib.rs](https://lib.rs/crates/eventsource-stream) -- 271K monthly downloads, Event struct API, eventsource trait
- [reqwest-eventsource on docs.rs](https://docs.rs/reqwest-eventsource/latest/reqwest_eventsource/) -- EventSource wrapper, reconnection behavior
- [async-stream releases](https://github.com/tokio-rs/async-stream/releases) -- v0.3.6, October 2024
- [vLLM OpenAI-Compatible Server docs](https://docs.vllm.ai/en/stable/serving/openai_compatible_server/) -- stream_include_usage support
- [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility) -- partial compatibility, stream_options status unclear
- [futures StreamExt docs](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html) -- map(), then() combinators
- [axum Body::from_stream docs](https://docs.rs/axum/latest/axum/body/struct.Body.html) -- stream type requirements
- [tokio oneshot channel docs](https://docs.rs/tokio/latest/tokio/sync/oneshot/index.html) -- single-value notification pattern
- [Adam Chalmers: Static streams for faster async proxies](https://blog.adamchalmers.com/streaming-proxy/) -- stream proxy patterns in Rust
- [Tokio framing tutorial](https://tokio.rs/tokio/tutorial/framing) -- byte stream buffering patterns
