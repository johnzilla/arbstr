# Architecture Patterns: Streaming SSE Token Extraction

**Domain:** SSE stream interception and token extraction in Rust/axum/reqwest proxy
**Researched:** 2026-02-15
**Overall confidence:** HIGH (patterns verified against existing codebase, OpenAI SSE format well-documented, all components use crates already in Cargo.toml)

## Current Architecture (Baseline)

### How Streaming Responses Flow Today

```
Client                     arbstr                           Provider
  |                          |                                |
  |  POST /v1/chat/          |                                |
  |  completions             |                                |
  |  stream: true            |                                |
  |------------------------->|                                |
  |                          |  POST /chat/completions        |
  |                          |  stream: true                  |
  |                          |------------------------------->|
  |                          |                                |
  |                          |  200 OK (chunked)              |
  |                          |  text/event-stream             |
  |                          |<-------------------------------|
  |                          |                                |
  |                          |  Log entry written             |
  |                          |  (tokens: None, cost: None)    |
  |                          |                                |
  |  200 OK (chunked)        |                                |
  |  text/event-stream       |                                |
  |<-------------------------|                                |
  |                          |                                |
  |  data: {"choices":[...]} |  bytes_stream() chunks         |
  |  data: {"choices":[...]} |  pass-through via              |
  |  data: {"choices":[...]} |  Body::from_stream             |
  |  ...                     |  (debug log of usage if seen,  |
  |  data: [DONE]            |   but value is discarded)      |
  |<-------------------------|                                |
```

### What Happens in Code (handlers.rs:665-729)

The `handle_streaming_response` function currently:

1. Takes the `reqwest::Response` from the provider
2. Calls `.bytes_stream()` to get a `Stream<Item = Result<Bytes, reqwest::Error>>`
3. Maps each chunk through a closure that:
   - Parses UTF-8 lines looking for `data: ` prefixed lines
   - Attempts to extract usage JSON from each SSE data payload
   - **Logs usage via `tracing::debug!` but discards the values** (lines 688-694)
   - Passes bytes through unchanged
4. Wraps the stream in `Body::from_stream(stream)`
5. Returns `RequestOutcome` with `input_tokens: None, output_tokens: None, cost_sats: None`

### The Logging Gap

In `chat_completions` (lines 152-231), the streaming path:

1. Calls `execute_request` which returns a `RequestOutcome`
2. Builds a `RequestLog` using the outcome's `input_tokens` / `output_tokens` / `cost_sats`
3. Calls `spawn_log_write` to insert the log entry
4. Returns the streaming response to the client

The log entry is written **before the stream is consumed** because the response body hasn't been read yet -- it's just a stream wrapper. The tokens are always `None` for streaming requests.

### What the Database Has

The `requests` table already has columns for `input_tokens`, `output_tokens`, `cost_sats`, and `provider_cost_sats` -- all nullable INTEGER/REAL. Streaming requests insert with these as NULL. No schema change is needed to store the data; we just need to UPDATE the row after the stream completes.

## Recommended Architecture

### Design Decision: Insert-Then-Update Pattern

The fundamental challenge is temporal: headers and the initial log entry must be sent/written before the stream is consumed, but token counts are only available after the final SSE chunk arrives (or the stream ends).

**Two viable approaches:**

**Approach A -- Insert placeholder, UPDATE after stream completes (recommended):**
Log the request immediately with `input_tokens = NULL` (current behavior). After the stream ends, update the same row with extracted token counts and calculated cost. Uses the existing `correlation_id` to find the row.

**Approach B -- Log only after stream completes:**
Defer the entire log write until after the stream ends. Pro: single INSERT with complete data. Con: if the client disconnects mid-stream, no log entry exists at all -- you lose the record that a request was made.

**Recommendation: Approach A (insert-then-update).** The current behavior of logging immediately is correct -- it captures the request even if the stream fails partway through. Adding a post-stream UPDATE preserves this safety property while filling in the missing data. The two-write cost is negligible for SQLite with WAL mode.

### Design Decision: stream_options Injection vs. Passive Parsing

**Approach A -- Inject `stream_options: {"include_usage": true}` into the upstream request (recommended):**
Before forwarding to the provider, add `stream_options.include_usage = true` to the request JSON. This asks OpenAI-compatible providers to include a final usage chunk with `prompt_tokens` and `completion_tokens`. The proxy then parses only this final chunk.

**Approach B -- Count tokens manually by accumulating delta content:**
Parse every SSE chunk, accumulate `delta.content` text, and use a tokenizer to estimate token counts. This is complex, inaccurate (tokenizer must match the model), and adds a heavy dependency.

**Recommendation: Approach A (inject stream_options).** The OpenAI API supports `stream_options: {"include_usage": true}` which causes the provider to emit an extra chunk before `[DONE]` containing the authoritative token counts. This is the standard mechanism. It adds no new dependencies, is accurate, and requires parsing only the final chunk rather than accumulating all content.

**Provider compatibility note:** The `stream_options` field is part of the OpenAI Chat Completions API specification. Routstr providers expose an OpenAI-compatible API, so this should be supported. If a provider does not support `stream_options`, it will either ignore the field (safe -- no usage chunk, tokens remain NULL) or return a 400 error (handled by existing error paths). The proxy should handle both cases gracefully.

### Design Decision: Side-Channel Communication Pattern

The stream is consumed by the client (via axum's response body). The proxy needs to learn what the stream contained after it's done. This requires a side channel from the stream processing closure to a post-stream completion handler.

**Approach A -- Arc<Mutex<Option<Usage>>> shared state (recommended):**
Create a shared `Arc<Mutex<Option<StreamUsage>>>` before building the stream. The stream's map closure writes to it when it encounters usage data. A `tokio::spawn` task holds a reference and polls/awaits stream completion, then reads the captured value and performs the UPDATE.

**Approach B -- tokio::sync::oneshot channel:**
Create a oneshot channel. The stream closure sends usage data through the sender when the stream ends. A spawned task awaits the receiver. Pro: no mutex. Con: oneshot can only send once, and detecting "stream ended without usage" requires dropping the sender (which means putting it in the stream closure's state), adding complexity. Harder to handle the case where usage arrives in the final data chunk but the stream hasn't technically ended yet (there's still the `[DONE]` line and stream closure).

**Approach C -- Custom Stream wrapper struct implementing Stream trait:**
Build a `StreamInterceptor` struct that wraps the inner stream, implements `Stream<Item = Result<Bytes, E>>`, and captures state internally. On `poll_next` returning `None` (stream exhausted), trigger the database update. Pro: most elegant, no external synchronization. Con: implementing `Stream` manually with `Pin` and `poll_next` is verbose and error-prone in Rust.

**Recommendation: Approach A (Arc<Mutex<Option>>).** This is the same pattern already used in `retry.rs` for `Arc<Mutex<Vec<AttemptRecord>>>` -- the codebase has precedent for this exact technique. It's simple, works with the existing `.map()` stream combinator, and the mutex contention is zero (written once at end of stream, read once after stream completes).

### Component Diagram

```
                                  BEFORE STREAM STARTS
                                  ====================

ChatCompletionRequest              Injected field
+---------------------+    +---> stream_options: { include_usage: true }
| model: "gpt-4o"     |    |
| stream: true         |----+     Forwarded to provider
| messages: [...]      |
+---------------------+

                                  DURING STREAM
                                  =============

Provider              bytes_stream()          Stream map closure         Client
   |                       |                        |                       |
   |  data: {chunk}        |   Bytes                |                       |
   |---------------------->|----------------------->| parse SSE lines       |
   |                       |                        | if usage found:       |
   |                       |                        |   write to shared_usage
   |                       |                        | pass bytes through    |
   |                       |                        |---------------------->|
   |                       |                        |                       |
   |  data: {usage chunk}  |   Bytes                |                       |
   |---------------------->|----------------------->| usage found!          |
   |                       |                        | *shared_usage =       |
   |                       |                        |   Some(StreamUsage)   |
   |                       |                        |---------------------->|
   |                       |                        |                       |
   |  data: [DONE]         |   Bytes                |                       |
   |---------------------->|----------------------->| pass through          |
   |                       |                        |---------------------->|
   |                       |                        |                       |
   |  (stream ends)        |   None                 |                       |
   |                       |                        | (stream exhausted)    |
   |                       |                        |                       |

                                  AFTER STREAM ENDS
                                  =================

                    Completion task (tokio::spawn)
                    +----------------------------------------+
                    | await stream_done signal               |
                    | read shared_usage lock                 |
                    | if Some(usage):                        |
                    |   calculate cost via actual_cost_sats  |
                    |   UPDATE requests SET                  |
                    |     input_tokens = ?,                  |
                    |     output_tokens = ?,                 |
                    |     cost_sats = ?,                     |
                    |     latency_ms = ?                     |
                    |   WHERE correlation_id = ?             |
                    | else:                                  |
                    |   log warning (no usage in stream)     |
                    +----------------------------------------+
```

### New Components

| Component | Location | Type | Purpose |
|-----------|----------|------|---------|
| `StreamUsage` | `src/proxy/stream.rs` (new module) | struct | Holds extracted `prompt_tokens: u32`, `completion_tokens: u32` from the usage chunk |
| `build_intercepted_stream` | `src/proxy/stream.rs` | fn | Takes a `bytes_stream()`, returns `(Body, Arc<Mutex<Option<StreamUsage>>>)` -- the pass-through body and the shared usage capture |
| `parse_sse_usage` | `src/proxy/stream.rs` | fn | Parses a single SSE data payload string and returns `Option<StreamUsage>` if it contains a usage object |
| `SseLineBuffer` | `src/proxy/stream.rs` | struct | Buffers partial SSE lines across chunk boundaries (chunks from reqwest do not respect line boundaries) |
| `inject_stream_options` | `src/proxy/stream.rs` or `handlers.rs` | fn | Mutates a `ChatCompletionRequest` to set `stream_options.include_usage = true` |
| `spawn_stream_completion` | `src/proxy/stream.rs` or `handlers.rs` | fn | Spawns a task that waits for stream completion and performs the database UPDATE |
| `update_streaming_usage` | `src/storage/logging.rs` | fn | Executes `UPDATE requests SET input_tokens=?, output_tokens=?, cost_sats=?, latency_ms=? WHERE correlation_id=?` |

### Modified Components

| Component | File | Change |
|-----------|------|--------|
| `ChatCompletionRequest` | `src/proxy/types.rs` | Add `stream_options: Option<StreamOptions>` field with serde serialization |
| `handle_streaming_response` | `src/proxy/handlers.rs` | Replace inline stream parsing with call to `build_intercepted_stream`, return the shared usage handle |
| `RequestOutcome` | `src/proxy/handlers.rs` | Add `shared_usage: Option<Arc<Mutex<Option<StreamUsage>>>>` for streaming responses |
| `chat_completions` (streaming path) | `src/proxy/handlers.rs` | After returning the response, spawn a completion task that awaits stream end and calls `update_streaming_usage` |
| `send_to_provider` | `src/proxy/handlers.rs` | Call `inject_stream_options` on the request before forwarding when `is_streaming` is true |
| `proxy/mod.rs` | `src/proxy/mod.rs` | Add `pub mod stream;` |

### Unchanged Components

| Component | Why Unchanged |
|-----------|--------------|
| `retry.rs` | Streaming bypasses retry (existing design decision) |
| `selector.rs` / Router | Provider selection is the same regardless of streaming |
| `server.rs` / AppState | No new shared state needed at the app level |
| `config.rs` | No config changes needed |
| `error.rs` | No new error variants needed (stream parsing failures are warnings, not errors) |
| Database schema (`migrations/`) | Existing columns are nullable and can be UPDATEd -- no migration needed |

## Pattern Details

### Pattern 1: SSE Line Buffering Across Chunk Boundaries

**What:** reqwest's `bytes_stream()` delivers arbitrary byte chunks that do not align with SSE line boundaries. A single SSE event like `data: {"choices":[...]}\n\n` may be split across multiple chunks, or multiple events may arrive in a single chunk.

**Why this matters:** The current code (handlers.rs:676-698) iterates `text.lines()` on each chunk. This works most of the time because SSE events are small and often fit in a single TCP segment. But it silently fails when a chunk boundary falls in the middle of a JSON payload -- the `serde_json::from_str` parse will fail, and the usage data will be lost.

**Implementation:**
```rust
/// Buffers partial SSE lines across byte-stream chunk boundaries.
///
/// reqwest chunks do not respect SSE line boundaries. This buffer
/// accumulates bytes until a complete line (\n) is found, then
/// yields complete lines for parsing.
struct SseLineBuffer {
    partial: String,
}

impl SseLineBuffer {
    fn new() -> Self {
        Self { partial: String::new() }
    }

    /// Feed a chunk of bytes. Returns an iterator of complete lines.
    fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        let text = match std::str::from_utf8(chunk) {
            Ok(t) => t,
            Err(_) => return vec![], // Non-UTF8 chunk, skip
        };

        self.partial.push_str(text);

        let mut lines = Vec::new();
        while let Some(newline_pos) = self.partial.find('\n') {
            let line = self.partial[..newline_pos].to_string();
            self.partial = self.partial[newline_pos + 1..].to_string();
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }
}
```

**Confidence:** HIGH -- this is a standard buffered line reader pattern. The current code skips this and gets lucky because most SSE chunks are line-aligned, but it's not guaranteed.

### Pattern 2: Usage Extraction from Final SSE Chunk

**What:** When `stream_options.include_usage` is set, the OpenAI API emits an extra chunk before `data: [DONE]` with the following structure:

```
data: {"id":"chatcmpl-...","object":"chat.completion.chunk","created":...,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":12,"total_tokens":21}}

data: [DONE]
```

Key characteristics of this final usage chunk:
- `choices` is an empty array `[]`
- `usage` is a non-null object with `prompt_tokens`, `completion_tokens`, `total_tokens`
- It appears immediately before `data: [DONE]`
- On all prior chunks, `usage` is either absent or `null`

**Implementation:**
```rust
/// Extracted token counts from a streaming response's usage chunk.
#[derive(Debug, Clone)]
pub struct StreamUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Parse an SSE data payload for usage information.
/// Returns Some(StreamUsage) if this is the final usage chunk.
fn parse_sse_usage(data: &str) -> Option<StreamUsage> {
    if data == "[DONE]" {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(data).ok()?;
    let usage = parsed.get("usage").filter(|u| !u.is_null())?;
    let prompt = usage.get("prompt_tokens")?.as_u64()? as u32;
    let completion = usage.get("completion_tokens")?.as_u64()? as u32;
    Some(StreamUsage {
        prompt_tokens: prompt,
        completion_tokens: completion,
    })
}
```

**Confidence:** HIGH -- the existing `extract_usage` function in handlers.rs uses the same JSON structure. OpenAI's documentation confirms the format. The `stream_options` parameter is a documented part of the Chat Completions API.

### Pattern 3: stream_options Injection

**What:** Before forwarding a streaming request to the provider, inject `stream_options: {"include_usage": true}` to request the usage chunk.

**Implementation approach -- modify ChatCompletionRequest:**
```rust
// In types.rs
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
}

// Add to ChatCompletionRequest:
#[serde(skip_serializing_if = "Option::is_none")]
pub stream_options: Option<StreamOptions>,
```

Then in the streaming path, before forwarding:
```rust
// Ensure stream_options.include_usage is set
if request.stream.unwrap_or(false) {
    let opts = request.stream_options.get_or_insert(StreamOptions {
        include_usage: None,
    });
    opts.include_usage = Some(true);
}
```

**Why modify the type rather than inject at JSON level:** The request is already deserialized into `ChatCompletionRequest` and re-serialized via `.json(request)` in `send_to_provider`. Adding the field to the struct keeps the flow clean. If the client already set `stream_options`, we preserve their settings and just ensure `include_usage` is true.

**Edge case -- client sets `include_usage: false`:** We override to `true`. The client's application shouldn't depend on usage being absent from the stream, and the extra chunk is harmless (it has `choices: []` so content-processing clients ignore it).

**Confidence:** HIGH -- `stream_options` is a documented OpenAI API field. Routstr exposes an OpenAI-compatible API.

### Pattern 4: Shared Usage Capture via Arc<Mutex<Option<T>>>

**What:** The stream map closure captures a reference to shared state. When it encounters usage data, it writes to the shared state. After the stream completes, a spawned task reads the shared state.

**Implementation:**
```rust
use std::sync::{Arc, Mutex};

pub fn build_intercepted_stream(
    byte_stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
) -> (Body, Arc<Mutex<Option<StreamUsage>>>) {
    let shared_usage: Arc<Mutex<Option<StreamUsage>>> = Arc::new(Mutex::new(None));
    let usage_writer = shared_usage.clone();

    let mut line_buffer = SseLineBuffer::new();

    let intercepted = byte_stream.map(move |chunk| {
        match &chunk {
            Ok(bytes) => {
                for line in line_buffer.feed(bytes) {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Some(usage) = parse_sse_usage(data) {
                            tracing::debug!(
                                prompt_tokens = usage.prompt_tokens,
                                completion_tokens = usage.completion_tokens,
                                "Captured usage from streaming response"
                            );
                            *usage_writer.lock().unwrap() = Some(usage);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::error!(error = %e, "Error in streaming response");
            }
        }
        chunk.map_err(std::io::Error::other)
    });

    let body = Body::from_stream(intercepted);
    (body, shared_usage)
}
```

**Why Arc<Mutex<Option>> and not just Arc<Mutex<StreamUsage>>:** The `Option` distinguishes "no usage chunk was received" (None) from "usage was extracted" (Some). This matters for logging -- if the provider doesn't support `stream_options`, we want to know that tokens remain unknown rather than assuming zero.

**Precedent in codebase:** `retry.rs` uses `Arc<Mutex<Vec<AttemptRecord>>>` for exactly the same pattern -- shared mutable state between an async operation and its observer. The comment on line 296 of handlers.rs explains: "created before timeout so it survives cancellation."

**Confidence:** HIGH -- direct codebase precedent, standard Rust async pattern.

### Pattern 5: Post-Stream Database UPDATE

**What:** After the stream completes, update the previously-inserted log row with token counts and cost.

**Implementation in storage/logging.rs:**
```rust
/// Update a streaming request's log entry with token counts and cost
/// after the stream has completed.
pub async fn update_streaming_usage(
    pool: &SqlitePool,
    correlation_id: &str,
    input_tokens: u32,
    output_tokens: u32,
    cost_sats: f64,
    latency_ms: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE requests
         SET input_tokens = ?, output_tokens = ?, cost_sats = ?, latency_ms = ?
         WHERE correlation_id = ? AND streaming = TRUE"
    )
    .bind(input_tokens as i64)
    .bind(output_tokens as i64)
    .bind(cost_sats)
    .bind(latency_ms)
    .bind(correlation_id)
    .execute(pool)
    .await?;
    Ok(())
}
```

**Why UPDATE rather than DELETE+INSERT:** The row already has correct values for `timestamp`, `model`, `provider`, `policy`, `streaming`, `success`. Only the token/cost/latency fields need updating. UPDATE is atomic and preserves the row ID.

**Why include `streaming = TRUE` in WHERE:** Defense in depth -- only update rows that are actually streaming entries. Prevents accidental overwrites if correlation IDs were reused (they won't be with UUID v4, but the guard costs nothing).

**Confidence:** HIGH -- standard SQL UPDATE, sqlx query pattern already used in the codebase.

### Pattern 6: Stream Completion Detection and Spawned Task

**What:** Detect when the stream has been fully consumed by the client and trigger the database update.

**Challenge:** The stream is consumed by axum's response body machinery, not by our code. We can't `.await` the stream completion in the handler because the handler returns the response (with the body) to axum.

**Implementation approach -- wrap stream with completion signal:**
```rust
/// Spawn a task that waits for the stream to complete, then updates the database.
pub fn spawn_stream_completion(
    pool: SqlitePool,
    correlation_id: String,
    shared_usage: Arc<Mutex<Option<StreamUsage>>>,
    provider_input_rate: u64,
    provider_output_rate: u64,
    provider_base_fee: u64,
    start_time: std::time::Instant,
    done_rx: tokio::sync::oneshot::Receiver<()>,
) {
    tokio::spawn(async move {
        // Wait for stream completion signal
        let _ = done_rx.await;

        let latency_ms = start_time.elapsed().as_millis() as i64;

        // Read captured usage
        let usage = shared_usage.lock().unwrap().take();
        match usage {
            Some(u) => {
                let cost = crate::router::actual_cost_sats(
                    u.prompt_tokens,
                    u.completion_tokens,
                    provider_input_rate,
                    provider_output_rate,
                    provider_base_fee,
                );
                tracing::info!(
                    correlation_id = %correlation_id,
                    prompt_tokens = u.prompt_tokens,
                    completion_tokens = u.completion_tokens,
                    cost_sats = cost,
                    latency_ms = latency_ms,
                    "Streaming response completed, updating log"
                );
                if let Err(e) = update_streaming_usage(
                    &pool,
                    &correlation_id,
                    u.prompt_tokens,
                    u.completion_tokens,
                    cost,
                    latency_ms,
                ).await {
                    tracing::warn!(
                        correlation_id = %correlation_id,
                        error = %e,
                        "Failed to update streaming usage in database"
                    );
                }
            }
            None => {
                tracing::debug!(
                    correlation_id = %correlation_id,
                    "Stream completed without usage data (provider may not support stream_options)"
                );
            }
        }
    });
}
```

**Stream completion signal:** The completion signal comes from a `tokio::sync::oneshot` channel. The sender is held by the stream wrapper and dropped when the stream ends (either normally or on error/client disconnect). The approach:

```rust
let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

let intercepted = byte_stream
    .map(move |chunk| { /* usage extraction */ })
    .chain(futures::stream::once(async move {
        // This item is produced after the inner stream ends.
        // Drop the sender to signal completion.
        drop(done_tx);
        // Produce an empty result that won't be sent (stream is ending).
        // Actually, we need this to not produce a real item.
        // Better approach below.
    }));
```

**Actually, simpler approach -- drop-based signaling:**

Wrap the `done_tx` in a struct that sends on drop, and move it into the stream closure. When the stream is fully consumed (or dropped due to client disconnect), the closure's captured state is dropped, which triggers the signal:

```rust
struct CompletionSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for CompletionSignal {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}
```

Move the `CompletionSignal` into the stream's map closure. When the stream is fully consumed or the response body is dropped (client disconnect), Rust's drop semantics ensure the signal fires.

**Alternative -- no oneshot, just Arc<AtomicBool> + polling:**

This is simpler but involves polling (sleep loop), which is wasteful. The oneshot approach is event-driven and zero-cost when idle.

**Confidence:** HIGH -- tokio::sync::oneshot is a standard primitive, drop-based signaling is idiomatic Rust, `tokio::spawn` for fire-and-forget is already used in the codebase.

## Anti-Patterns to Avoid

### Anti-Pattern 1: Buffering the Entire Stream in Memory

**What:** Collecting all SSE chunks into a Vec<Bytes>, parsing usage at the end, then re-streaming to the client.

**Why bad:** Defeats the purpose of streaming. The client would see no output until the entire response is generated. For long responses, this adds seconds of perceived latency. Also doubles memory usage.

**Instead:** Pass-through streaming with side-channel capture. The client receives bytes in real-time; usage is extracted as a side effect.

### Anti-Pattern 2: Manual Token Counting via Tokenizer

**What:** Adding a tokenizer dependency (like `tiktoken-rs`) to count tokens from accumulated delta content.

**Why bad:** Heavy dependency (~20MB for tiktoken data), must match the exact tokenizer for each model (GPT-4o uses cl100k_base, Claude uses a different one), inaccurate for non-text content (tool calls, structured output). The provider already knows the exact count.

**Instead:** Use `stream_options.include_usage = true` to get authoritative token counts from the provider.

### Anti-Pattern 3: Parsing Every Chunk as Complete JSON

**What:** Treating each `Bytes` chunk from reqwest as a complete, self-contained SSE message.

**Why bad:** TCP chunks do not respect SSE message boundaries. A chunk might contain half a JSON payload, or multiple complete messages. `serde_json::from_str` on a truncated payload silently fails, and the usage data in that chunk is lost.

**Instead:** Buffer partial lines with `SseLineBuffer` and only parse complete lines.

### Anti-Pattern 4: Blocking the Response on Stream Completion

**What:** Trying to `.await` the stream in the handler before returning the response.

**Why bad:** The handler must return the `Response` (containing the stream body) for axum to start sending to the client. If you await the stream first, you've buffered everything (see Anti-Pattern 1). The HTTP response headers are sent before any body bytes, so the handler must return promptly.

**Instead:** Return the response immediately, spawn a background task that awaits the stream completion signal.

### Anti-Pattern 5: Using request_id Header for Post-Stream Correlation

**What:** Sending a custom header to the client with the correlation ID and expecting the client to send it back to trigger the update.

**Why bad:** The client is not part of this architecture. The proxy manages its own state. Adding client-side requirements breaks OpenAI API compatibility.

**Instead:** Internal correlation via the existing `correlation_id` (UUID v4) stored in the database row.

## Data Flow (After Changes)

### Happy Path: Provider Supports stream_options

```
1. Client sends: POST /v1/chat/completions, stream: true

2. chat_completions handler:
   -> Detects streaming
   -> Calls execute_request

3. execute_request -> send_to_provider:
   -> inject_stream_options: adds stream_options.include_usage = true
   -> POST to provider with modified request

4. Provider responds: 200 OK, text/event-stream

5. handle_streaming_response:
   -> Creates shared_usage = Arc<Mutex<Option<StreamUsage>>>(None)
   -> Creates (done_tx, done_rx) oneshot channel
   -> Calls build_intercepted_stream(response.bytes_stream())
   -> Returns RequestOutcome with body and shared_usage handle

6. chat_completions handler:
   -> Writes initial log entry (tokens: None, cost: None) -- existing behavior
   -> Spawns stream_completion task with (pool, correlation_id, shared_usage, rates, done_rx)
   -> Returns streaming response to client

7. Client receives SSE chunks in real-time (zero latency added):
   data: {"choices":[{"delta":{"content":"Hello"}}],"usage":null}
   data: {"choices":[{"delta":{"content":" world"}}],"usage":null}
   ...

8. Stream map closure processes each chunk:
   -> SseLineBuffer buffers partial lines
   -> parse_sse_usage finds no usage in content chunks (usage is null)
   -> Bytes pass through to client unchanged

9. Provider sends final usage chunk:
   data: {"choices":[],"usage":{"prompt_tokens":15,"completion_tokens":42,"total_tokens":57}}
   data: [DONE]

10. Stream map closure processes usage chunk:
    -> parse_sse_usage returns Some(StreamUsage { prompt: 15, completion: 42 })
    -> *shared_usage.lock() = Some(StreamUsage { ... })

11. Stream ends (reqwest stream returns None):
    -> CompletionSignal is dropped -> done_tx sends ()
    -> Stream body reports complete to axum

12. Spawned completion task wakes up:
    -> done_rx receives ()
    -> Reads shared_usage: Some(StreamUsage { prompt: 15, completion: 42 })
    -> Calculates cost: actual_cost_sats(15, 42, input_rate, output_rate, base_fee)
    -> UPDATE requests SET input_tokens=15, output_tokens=42,
         cost_sats=X, latency_ms=Y WHERE correlation_id='...'

13. Database now has complete record for this streaming request
```

### Degraded Path: Provider Does Not Support stream_options

```
1-6. Same as happy path

7. Provider ignores stream_options, sends normal chunks without usage:
   data: {"choices":[{"delta":{"content":"Hello"}}]}
   data: {"choices":[{"delta":{"content":" world"}}]}
   data: [DONE]

8-9. Stream map closure processes chunks:
   -> No usage object found in any chunk
   -> shared_usage remains None

10. Stream ends, completion task fires:
    -> Reads shared_usage: None
    -> Logs: "Stream completed without usage data"
    -> No UPDATE performed
    -> Database row retains input_tokens=NULL, output_tokens=NULL, cost_sats=NULL

11. This is the CURRENT behavior -- no regression
```

### Error Path: Client Disconnects Mid-Stream

```
1-6. Same as happy path

7. Client disconnects after receiving some chunks

8. axum drops the response body -> stream is dropped
   -> CompletionSignal is dropped -> done_tx sends ()
   -> OR: done_tx is dropped without sending if signal struct is dropped
         (oneshot receiver gets RecvError, task handles gracefully)

9. Spawned completion task wakes up:
   -> Reads shared_usage: None (usage chunk hadn't arrived yet)
   -> Logs: "Stream completed without usage data"
   -> No UPDATE (tokens remain NULL)
   -> Original INSERT still present with success=true
     (NOTE: success reflects provider response status, not stream completion)

10. This is acceptable -- the request was partially delivered.
    The database shows a streaming request with unknown token count.
```

## Integration Points with Existing Code

### 1. types.rs: Add StreamOptions to ChatCompletionRequest

**Current:** `ChatCompletionRequest` has no `stream_options` field.
**Change:** Add `stream_options: Option<StreamOptions>` with `#[serde(skip_serializing_if = "Option::is_none")]`.
**Risk:** LOW -- additive field with skip_serializing_if, backward compatible. Existing requests without `stream_options` deserialize as `None`.

### 2. handlers.rs: send_to_provider Injects stream_options

**Current:** `send_to_provider` takes `&ChatCompletionRequest` (immutable reference).
**Change:** Either change to `&mut ChatCompletionRequest` or clone-and-mutate before calling. Since the request is already cloned for the retry path, mutating a clone is natural.
**Risk:** LOW -- the mutation happens before serialization to JSON.

### 3. handlers.rs: handle_streaming_response Returns Usage Handle

**Current:** Returns `RequestOutcome` with `input_tokens: None`.
**Change:** Returns `RequestOutcome` with a `shared_usage` handle. The `RequestOutcome` struct gains an optional field.
**Risk:** LOW -- additive field, non-streaming paths set it to `None`.

### 4. handlers.rs: chat_completions Spawns Completion Task

**Current:** Streaming path logs then returns.
**Change:** After logging, spawns a `stream_completion` task if `shared_usage` is present.
**Risk:** LOW -- additive. The spawned task is fire-and-forget like existing `spawn_log_write`. Failure is logged as a warning.

### 5. storage/logging.rs: New update_streaming_usage Function

**Current:** Only has `RequestLog::insert` and `spawn_log_write`.
**Change:** Add `update_streaming_usage` function and `spawn_update_streaming_usage` convenience wrapper.
**Risk:** LOW -- new function, doesn't modify existing code.

### 6. proxy/mod.rs: New stream Module

**Current:** `mod.rs` declares `handlers`, `retry`, `server`, `types`.
**Change:** Add `pub mod stream;`.
**Risk:** NONE -- additive.

## Build Order (Dependency Graph)

```
Step 1: src/proxy/types.rs
   |     Add StreamOptions struct
   |     Add stream_options field to ChatCompletionRequest
   |     (Fully backward compatible, no other code changes needed)
   |
   v
Step 2: src/proxy/stream.rs (NEW MODULE) + src/proxy/mod.rs
   |     - SseLineBuffer (line buffering)
   |     - StreamUsage struct
   |     - parse_sse_usage() function
   |     - build_intercepted_stream() function
   |     - CompletionSignal (drop-based oneshot trigger)
   |     - Unit tests for SseLineBuffer (partial chunks, multi-line chunks)
   |     - Unit tests for parse_sse_usage (usage chunk, content chunk, [DONE])
   |     (Fully testable in isolation, no handler changes yet)
   |
   v
Step 3: src/storage/logging.rs
   |     - update_streaming_usage() async function
   |     - spawn_update_streaming_usage() fire-and-forget wrapper
   |     (Testable in isolation with a test database)
   |
   v
Step 4: src/proxy/handlers.rs -- THE INTEGRATION
   |     - RequestOutcome gains shared_usage field
   |     - send_to_provider calls inject_stream_options for streaming
   |     - handle_streaming_response uses build_intercepted_stream
   |     - chat_completions streaming path spawns completion task
   |     (Wires everything together, existing tests must still pass)
   |
   v
Step 5: Integration testing
         - Mock provider that emits SSE with usage chunk
         - Verify database row is updated after stream completes
         - Test partial chunk buffering with realistic SSE data
         - Test client disconnect handling
```

**Why this order:**
- Step 1 is purely additive: new serde field, no behavioral change, all existing tests pass
- Step 2 is the core new logic, fully unit-testable without touching handlers
- Step 3 is the database function, testable with a real SQLite database
- Step 4 is the integration point, where compiler errors guide wiring
- Step 5 validates the complete flow end-to-end

**Dependency chain:** Steps 1-3 are independent of each other and could be built in parallel. Step 4 depends on all three. Step 5 depends on Step 4.

## Scalability Considerations

| Concern | Impact | Assessment |
|---------|--------|------------|
| One extra Mutex lock per SSE chunk | ~100ns per chunk, 50-200 chunks per response | Negligible. Lock is uncontended (single writer). |
| SseLineBuffer allocation | One String buffer per stream | Negligible. Buffer is small (SSE lines are typically < 1KB). Freed when stream ends. |
| Extra database UPDATE per streaming request | One SQLite write after stream completes | Negligible. WAL mode handles concurrent reads during write. Fire-and-forget, non-blocking. |
| tokio::spawn for completion task | One lightweight task per streaming request | Negligible. Task is mostly idle (awaiting oneshot), then does one DB write. Same pattern as existing spawn_log_write. |
| stream_options injection | One field added to JSON payload | Negligible. A few extra bytes in the request body. |

**At scale:** Even at 100 concurrent streaming requests, the overhead is 100 Mutex locks (uncontended), 100 oneshot channels, and 100 extra SQLite UPDATEs. This is well within SQLite's capabilities with WAL mode.

## Sources

- [OpenAI Streaming API Reference](https://platform.openai.com/docs/api-reference/chat-streaming) -- stream_options parameter, usage chunk format, choices=[] on final chunk
- [OpenAI Usage Stats Announcement](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156) -- include_usage feature availability
- [OpenAI Streaming Cookbook](https://developers.openai.com/cookbook/examples/how_to_stream_completions/) -- practical examples of stream_options usage
- [axum Body::from_stream docs](https://docs.rs/axum/latest/axum/body/struct.Body.html) -- Body construction from async streams
- [axum SSE Middleware Discussion](https://github.com/tokio-rs/axum/discussions/2728) -- patterns for intercepting SSE streams in middleware
- [Streaming Proxy Blog (Adam Chalmers)](https://blog.adamchalmers.com/streaming-proxy/) -- Rust async streaming proxy patterns
- [futures StreamExt docs](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html) -- map, chain, and other stream combinators
- [tokio oneshot channel docs](https://docs.rs/tokio/latest/tokio/sync/oneshot/fn.channel.html) -- completion signaling
- [Rust Forum: reqwest byte stream line buffering](https://users.rust-lang.org/t/tokio-reqwest-byte-stream-to-lines/65258) -- chunk boundary issues
- Direct codebase analysis of arbstr src/ (handlers.rs streaming path, retry.rs Arc<Mutex> pattern, logging.rs fire-and-forget pattern, types.rs request structure)
