# Phase 9: SSE Stream Interception - Research

**Researched:** 2026-02-16
**Domain:** SSE line buffering, stream observation/tee pattern, usage extraction from OpenAI-compatible streaming responses, panic isolation in Rust
**Confidence:** HIGH

## Summary

Phase 9 builds a standalone stream wrapper module that buffers SSE lines across TCP chunk boundaries and extracts usage data from the final chunk. This is an observation-only module -- all bytes pass through unmodified to the client. The module produces a structured result after the stream completes that Phase 10 will consume for database updates and trailing cost events.

The core technical challenge is correctly reassembling SSE data lines that arrive split across arbitrary TCP chunk boundaries, while maintaining zero-copy observation of the stream (no content mutation, no cloning of chunk bytes). The existing codebase already has a stream adapter pattern in `handle_streaming_response` (handlers.rs:674-716) that processes each `Bytes` chunk via `.map()`. This pattern must be extended with a cross-chunk line buffer. The extracted usage data -- `prompt_tokens`, `completion_tokens`, and `finish_reason` -- is communicated back via a shared `Arc<Mutex<_>>` that the caller reads after the stream ends.

The second challenge is panic isolation: a bug in the extraction logic must never crash the client's stream. Rust's `std::panic::catch_unwind` works for the synchronous closures used in the stream's `.map()` adapter. The `futures` crate also provides `StreamExt::catch_unwind()` as a stream-level combinator. Either approach can protect the passthrough.

**Primary recommendation:** Create a new `src/proxy/stream.rs` module containing: (1) a `StreamResult` struct with `Option<Usage>`, `Option<String>` finish_reason, and `bool` done_received; (2) a `SseObserver` that encapsulates the line buffer and extraction state; (3) a `wrap_sse_stream` function that takes the upstream `bytes_stream()`, wraps it with the observation layer, and returns both the wrapped stream (for `Body::from_stream`) and a shared handle for reading the result after the stream ends. Use `Vec<u8>` for the line buffer, `std::str::from_utf8` for zero-copy parsing, and `std::panic::catch_unwind` around the extraction logic in the per-chunk closure.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Extraction scope:**
- Extract usage object (prompt_tokens, completion_tokens) AND finish_reason from the final chunk
- Do NOT extract model or other metadata -- just usage + finish_reason
- Return data only after the stream completes, not during streaming
- Return a structured result type (e.g., StreamResult { usage, finish_reason, done_received })
- Track whether `[DONE]` was received as part of the result -- Phase 10 uses this to know the stream completed normally

**Provider variation:**
- Strict OpenAI SSE format only -- no fallback parsing for non-standard usage locations
- If a provider deviates from OpenAI format, treat as "no usage data" -- safe degradation
- Skip non-`data:` SSE lines (event:, id:, retry:) but log them at trace level for debugging
- No fallback parsing -- if usage isn't where OpenAI puts it, report no usage
- Handle both `\n` and `\r\n` line endings in the SSE buffer

**Failure behavior:**
- If JSON parse fails on a `data:` line mid-stream, skip the bad line and continue trying -- usage may still arrive in the final chunk
- Log extraction issues (malformed JSON, missing fields, unexpected format) at warn level
- If stream ends without `[DONE]` (provider disconnect, timeout), return empty result -- without `[DONE]`, data is unreliable, avoid bad accounting
- Isolate extraction from the stream passthrough -- catch panics so a bug in extraction never breaks the client stream

### Claude's Discretion
- Internal buffer implementation (Vec<u8>, BufRead, custom ring buffer)
- Exact structured result type naming and field types
- How to implement panic isolation in Rust (catch_unwind, separate task, etc.)
- Test fixture design for chunk-boundary scenarios

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

## Standard Stack

### Core (already in Cargo.toml -- no additions needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| futures | 0.3.31 | `StreamExt::map()` for stream observation, `StreamExt::catch_unwind()` available | Already used in handlers.rs for `StreamExt` |
| serde_json | 1 | Parse SSE data lines as `serde_json::Value` to extract usage fields | Already a dependency |
| bytes | 1.11.0 | `Bytes` type from reqwest `bytes_stream()`, zero-copy `&[u8]` access | Transitive dependency via axum/reqwest |
| tracing | 0.1 | `trace!` for non-data SSE lines, `warn!` for parse failures | Already used everywhere |
| tokio | 1 | `Arc`, stream runtime | Already the async runtime |

### Supporting (no new additions)

No new crate dependencies needed. The standard library provides `std::panic::catch_unwind` and `std::panic::AssertUnwindSafe`. The `futures` crate provides `StreamExt` combinators.

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-rolled line buffer (`Vec<u8>`) | `eventsource-stream` crate | Full SSE parsing crate; adds a dependency for a simple line-splitting task. We only need line buffering + `data:` prefix stripping, not full event semantics |
| `Vec<u8>` buffer | `String` buffer | `String` requires valid UTF-8 on every append; `Vec<u8>` defers UTF-8 validation to line processing time, which is more resilient to partial multi-byte chars at chunk boundaries |
| `std::panic::catch_unwind` in `.map()` closure | `StreamExt::catch_unwind()` on the whole stream | `catch_unwind()` on the stream terminates the stream on panic (makes it the final item); per-closure `catch_unwind` allows the stream to continue after a panic in extraction logic -- better for the "never break the client stream" requirement |
| `Arc<Mutex<Option<StreamResult>>>` | `tokio::sync::watch` channel | `watch` is overkill for a single write at stream end; `Arc<Mutex<_>>` is simpler and the existing codebase already uses this pattern |

## Architecture Patterns

### Recommended Module Structure

```
src/proxy/
├── mod.rs            # ADD: pub mod stream;
├── stream.rs         # NEW: SSE observation module (this phase)
├── handlers.rs       # UNCHANGED (Phase 10 wires stream.rs into here)
├── server.rs         # UNCHANGED
├── retry.rs          # UNCHANGED
└── types.rs          # UNCHANGED
```

### Pattern 1: StreamResult Structured Type

**What:** The structured result that the observer produces after the stream completes.
**When to use:** Consumed by Phase 10 to drive database UPDATE and trailing cost event.

```rust
/// Token usage extracted from the final SSE chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Result of observing an SSE stream to completion.
#[derive(Debug, Clone)]
pub struct StreamResult {
    /// Token usage from the final chunk's usage object, if present.
    pub usage: Option<StreamUsage>,
    /// The finish_reason from the last chunk with a non-empty choices array.
    pub finish_reason: Option<String>,
    /// Whether the `data: [DONE]` sentinel was received.
    /// Phase 10 uses this as the trust signal -- without [DONE],
    /// the stream did not complete normally.
    pub done_received: bool,
}

impl StreamResult {
    /// An empty result for streams that ended without [DONE].
    pub fn empty() -> Self {
        Self {
            usage: None,
            finish_reason: None,
            done_received: false,
        }
    }
}
```

**Design rationale:**
- `StreamUsage` is a separate struct (not reusing `proxy::types::Usage`) because it omits `total_tokens` -- the caller can compute that if needed.
- `finish_reason` is `Option<String>` because it may not be present (provider omits it) or the stream may end without one.
- `done_received` is the critical trust signal per the locked decision: "If stream ends without [DONE], return empty result."

### Pattern 2: SseObserver (Internal Extraction State)

**What:** The per-stream state machine that buffers lines and extracts data.
**When to use:** Created per-stream, held inside the `.map()` closure.

```rust
/// Internal state for SSE line buffering and usage extraction.
struct SseObserver {
    /// Byte buffer for reassembling SSE lines across chunk boundaries.
    buffer: Vec<u8>,
    /// Extracted usage from the last chunk that had a non-null usage object.
    usage: Option<StreamUsage>,
    /// Extracted finish_reason from the last chunk with non-empty choices.
    finish_reason: Option<String>,
    /// Whether `data: [DONE]` was received.
    done_received: bool,
}

impl SseObserver {
    fn new() -> Self { /* ... */ }

    /// Process a chunk of bytes. Appends to the internal buffer,
    /// extracts complete lines, and parses data lines.
    fn process_chunk(&mut self, bytes: &[u8]) { /* ... */ }

    /// Build the final result. If [DONE] was not received,
    /// returns an empty result per the locked decision.
    fn into_result(self) -> StreamResult {
        if !self.done_received {
            return StreamResult::empty();
        }
        StreamResult {
            usage: self.usage,
            finish_reason: self.finish_reason,
            done_received: true,
        }
    }
}
```

**Key behaviors:**
1. `process_chunk` appends bytes to `buffer`, scans for complete lines (`\n` or `\r\n` terminated), processes each complete line, retains any trailing partial line.
2. For each complete line: skip blank lines, skip lines starting with `:` / `event:` / `id:` / `retry:` (log at trace), process `data:` lines.
3. For `data:` lines: if `data: [DONE]`, set `done_received = true`. Otherwise, parse as JSON (`serde_json::Value`), extract `usage` and `finish_reason` if present.
4. Usage extraction: look for `value["usage"]` that is not null, extract `prompt_tokens` and `completion_tokens`.
5. Finish reason extraction: look for `value["choices"][0]["finish_reason"]` that is a non-null string.

### Pattern 3: wrap_sse_stream Function (Public API)

**What:** The public function that wraps an upstream byte stream with the observation layer.
**When to use:** Called by Phase 10's handler integration to wrap `upstream_response.bytes_stream()`.

```rust
use bytes::Bytes;
use futures::Stream;
use std::sync::{Arc, Mutex};

/// Handle for reading the stream result after the stream completes.
pub type StreamResultHandle = Arc<Mutex<Option<StreamResult>>>;

/// Wrap an SSE byte stream with an observation layer.
///
/// Returns:
/// - A wrapped stream that passes all bytes through unmodified
/// - A handle to read the extracted StreamResult after the stream ends
///
/// The observation layer buffers SSE lines across chunk boundaries
/// and extracts usage data. Panics in extraction logic are caught
/// and do not affect the byte passthrough.
pub fn wrap_sse_stream<S>(stream: S) -> (impl Stream<Item = Result<Bytes, std::io::Error>>, StreamResultHandle)
where
    S: Stream<Item = Result<Bytes, reqwest::Error>>,
{
    let result_handle: StreamResultHandle = Arc::new(Mutex::new(None));
    let handle_clone = result_handle.clone();
    let observer = Arc::new(Mutex::new(SseObserver::new()));

    let wrapped = stream.map(move |chunk_result| {
        match chunk_result {
            Ok(ref bytes) => {
                // Panic-isolated extraction
                let observer_ref = observer.clone();
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    observer_ref.lock().unwrap().process_chunk(bytes);
                }));
            }
            Err(ref e) => {
                tracing::error!(error = %e, "Error streaming from provider");
            }
        }
        chunk_result.map_err(std::io::Error::other)
    });

    // ... finalization logic (see Pattern 4)
    (wrapped, handle_clone)
}
```

**Key design decisions:**
- The observer is `Arc<Mutex<SseObserver>>` because the `.map()` closure captures it by move, but we also need to read it after the stream ends.
- `std::panic::catch_unwind` wraps only the extraction call, not the bytes passthrough. A panic in `process_chunk` is caught and logged; the bytes still flow to the client.
- The error type conversion (`reqwest::Error` to `std::io::Error`) matches the existing pattern at handlers.rs:715.

### Pattern 4: Stream Finalization (Writing Result on Stream End)

**What:** Detecting when the stream has been fully consumed and writing the extracted result to the shared handle.
**When to use:** When the last chunk has been yielded and the caller polls for more items.

The cleanest approach: chain a finalizer onto the stream.

```rust
use futures::StreamExt;

// After the main stream, append a finalizer that writes the result
let observer_for_final = observer.clone();
let handle_for_final = handle_clone.clone();

let finalized = wrapped.chain(futures::stream::once(async move {
    // Stream has ended -- finalize
    let result = observer_for_final
        .lock()
        .unwrap()
        .take_result();  // consumes observer state
    *handle_for_final.lock().unwrap() = Some(result);
    // This item is never yielded to the client because it returns Err
    // (or we can use a filter approach)
    // Alternative: use .then() on the last item
}));
```

**Better approach:** Use `inspect` or a custom wrapper that detects `None` (stream end):

```rust
// Simpler: write result in a Drop impl on the observer
impl Drop for SseObserver {
    fn drop(&mut self) {
        // Build and store result when observer is dropped
        // (happens when stream is dropped = consumed or abandoned)
    }
}
```

**Recommended approach:** The `SseObserver` holds an `Option<StreamResultHandle>`. When the observer is dropped (stream consumed or abandoned), `Drop::drop` writes the result to the handle. This is simpler than chaining stream items and works for both normal completion and client disconnect.

```rust
struct SseObserver {
    buffer: Vec<u8>,
    usage: Option<StreamUsage>,
    finish_reason: Option<String>,
    done_received: bool,
    result_handle: StreamResultHandle,
}

impl Drop for SseObserver {
    fn drop(&mut self) {
        let result = if self.done_received {
            StreamResult {
                usage: self.usage.take(),
                finish_reason: self.finish_reason.take(),
                done_received: true,
            }
        } else {
            StreamResult::empty()
        };
        *self.result_handle.lock().unwrap() = Some(result);
    }
}
```

This approach:
- Works for normal stream completion (all items consumed, stream dropped)
- Works for client disconnect (axum drops the response body, which drops the stream, which drops the observer)
- Is simpler than `chain()` or other stream combinator approaches
- Avoids yielding spurious items to the client

### Pattern 5: Line Buffer Algorithm

**What:** The core algorithm for reassembling SSE lines across chunk boundaries.
**When to use:** Inside `SseObserver::process_chunk`.

```rust
fn process_chunk(&mut self, bytes: &[u8]) {
    self.buffer.extend_from_slice(bytes);

    // Process all complete lines in the buffer
    loop {
        // Find the next newline (\n or \r\n)
        let newline_pos = self.buffer.iter().position(|&b| b == b'\n');
        let Some(pos) = newline_pos else {
            break; // No complete line yet, wait for more data
        };

        // Extract the line (excluding the \n)
        let line_end = if pos > 0 && self.buffer[pos - 1] == b'\r' {
            pos - 1  // Handle \r\n
        } else {
            pos       // Handle \n
        };

        let line_bytes = &self.buffer[..line_end];

        // Process the line (only if it's valid UTF-8)
        if let Ok(line) = std::str::from_utf8(line_bytes) {
            self.process_line(line);
        } else {
            tracing::warn!("Non-UTF8 SSE line, skipping");
        }

        // Remove processed bytes from buffer (including the \n)
        self.buffer.drain(..=pos);
    }
}

fn process_line(&mut self, line: &str) {
    if line.is_empty() {
        // Blank line = SSE event delimiter, skip
        return;
    }

    if line.starts_with(':') {
        tracing::trace!(line = line, "SSE comment line");
        return;
    }

    if line.starts_with("event:") || line.starts_with("id:") || line.starts_with("retry:") {
        tracing::trace!(field = line, "SSE non-data field");
        return;
    }

    if let Some(data) = line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:")) {
        self.process_data(data);
    }
    // Lines that don't match any prefix are ignored (per SSE spec,
    // lines with unrecognized field names are skipped)
}

fn process_data(&mut self, data: &str) {
    let data = data.trim();

    if data == "[DONE]" {
        self.done_received = true;
        return;
    }

    // Parse as JSON
    let parsed: serde_json::Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse SSE data line as JSON");
            return;
        }
    };

    // Extract finish_reason from choices[0].finish_reason
    if let Some(reason) = parsed
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(|r| r.as_str())
    {
        self.finish_reason = Some(reason.to_string());
    }

    // Extract usage (only from chunks where usage is non-null)
    if let Some(usage) = parsed.get("usage").filter(|u| !u.is_null()) {
        if let (Some(prompt), Some(completion)) = (
            usage.get("prompt_tokens").and_then(|v| v.as_u64()),
            usage.get("completion_tokens").and_then(|v| v.as_u64()),
        ) {
            self.usage = Some(StreamUsage {
                prompt_tokens: prompt as u32,
                completion_tokens: completion as u32,
            });
        } else {
            tracing::warn!("Usage object present but missing expected fields");
        }
    }
}
```

**Key implementation details:**
- Buffer is `Vec<u8>`, not `String`. This avoids UTF-8 validation on every chunk append. UTF-8 is validated per-line.
- `drain(..=pos)` removes processed bytes including the newline. Only the trailing partial line remains.
- Both `data: ` (with space) and `data:` (without space) are handled per the SSE spec ("if value starts with a space, remove it").
- `finish_reason` is extracted from every chunk that has one, keeping the last seen. In a normal stream, `finish_reason: "stop"` appears on the last content chunk (before the usage chunk).
- `usage` is extracted from any chunk where the `usage` field is non-null. In OpenAI format, only the final chunk (with `choices: []`) has non-null usage.

### Anti-Patterns to Avoid

- **Modifying the bytes:** The stream must pass all bytes through unmodified. Never filter, reorder, or transform chunks. The observation is read-only.
- **Using `String` buffer with `push_str` per chunk:** If a chunk boundary falls inside a multi-byte UTF-8 character, `std::str::from_utf8(chunk)` would fail, and the partial character would be lost. Use `Vec<u8>` and validate UTF-8 per complete line.
- **Cloning `Bytes` for parsing:** `bytes::Bytes` supports zero-cost reference counting via `clone()`, but converting to `String` or `Vec<u8>` allocates. Use `std::str::from_utf8(&bytes)` to borrow without allocation.
- **Using `text.lines()` per chunk:** This is the existing bug in handlers.rs that Phase 9 fixes. `lines()` does not handle cross-chunk line continuations.
- **Deserializing into `ChatCompletionChunk` struct:** The existing `ChatCompletionChunk` type (types.rs:87-95) does not have a `usage` field. Using `serde_json::Value` for extraction is correct because it handles any JSON shape gracefully.
- **Holding `MutexGuard` across await points:** The `.map()` closure is synchronous, so this is not a risk here. But if refactored to `.then()` (async), it would deadlock with `std::sync::Mutex`.
- **Returning partial data when `[DONE]` was not received:** Per locked decision, no `[DONE]` means unreliable data. Return `StreamResult::empty()`.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| SSE line splitting | Custom byte scanner from scratch | `Vec<u8>` buffer with `position()` scan for `\n` | The algorithm is simple but must handle `\r\n` and UTF-8 boundaries correctly. Using `position()` on the buffer is efficient and correct |
| Stream observation combinator | Custom `Stream` impl with `Pin<Box<dyn Stream>>` | `futures::StreamExt::map()` closure with captured state | The `.map()` pattern already works in the codebase (handlers.rs:680). Extending it with state is simpler than a custom Stream impl |
| Panic isolation | Spawning a separate tokio task per stream | `std::panic::catch_unwind` in the synchronous `.map()` closure | `catch_unwind` is lightweight (no task spawn overhead, no channel), catches panics synchronously, and the closure is already synchronous |
| Stream end detection | `StreamExt::chain()` with a finalizer item | `Drop` impl on `SseObserver` | Drop-based finalization is simpler, handles both normal completion and client disconnect, and does not inject spurious items into the client's stream |

**Key insight:** The existing `.map()` closure pattern in the codebase is the right foundation. Phase 9 adds a `Vec<u8>` buffer for cross-chunk line reassembly and shared state for communicating the result. No fundamentally new patterns are needed.

## Common Pitfalls

### Pitfall 1: SSE Lines Split Across TCP Chunk Boundaries

**What goes wrong:** The current code calls `text.lines()` on each chunk independently, assuming complete SSE lines per chunk. A usage line like `data: {"usage":{"prompt_tokens":100,"completion_tokens":200}}` can be split at any byte boundary across two or more TCP chunks.
**Why it happens:** `reqwest::Response::bytes_stream()` yields chunks at TCP/HTTP chunked transfer boundaries, not SSE event boundaries. Developers test on localhost where chunks tend to be large.
**How to avoid:** Maintain a `Vec<u8>` buffer across chunks. Only process complete lines (terminated by `\n`). Keep trailing partial lines for the next chunk.
**Warning signs:** Usage extraction works on localhost but shows NULL in production logs.

### Pitfall 2: Buffer Memory Growth

**What goes wrong:** If the buffer accumulates all processed data without draining, memory grows linearly with response size.
**Why it happens:** `extend_from_slice` without corresponding `drain`.
**How to avoid:** After extracting complete lines, `drain(..=last_newline_pos)` removes processed bytes. Only the trailing incomplete line remains. For a typical SSE line (~200 bytes), the buffer holds at most one partial line.
**Warning signs:** Memory usage correlated with response length.

### Pitfall 3: Non-UTF8 Bytes from Misbehaving Provider

**What goes wrong:** A provider sends non-UTF8 bytes (e.g., `0xFF` in a binary error message). If the buffer is a `String`, `push_str(from_utf8(&bytes).unwrap())` panics.
**Why it happens:** Not all providers send clean UTF-8 in all error conditions.
**How to avoid:** Use `Vec<u8>` buffer. Validate UTF-8 per complete line with `std::str::from_utf8`, which returns `Err` instead of panicking. Skip non-UTF8 lines with a warning.
**Warning signs:** Panics in the stream closure under error conditions.

### Pitfall 4: `[DONE]` Without Trailing Newline

**What goes wrong:** Some providers send `data: [DONE]` as the very last bytes without a trailing `\n`. The buffer holds the complete line but the newline scanner never finds `\n`, so the line is never processed.
**Why it happens:** The `[DONE]` sentinel is the last thing sent before TCP FIN. Some servers omit the final newline.
**How to avoid:** In the `Drop` impl (or finalization), process any remaining buffer content as a final line, even without a trailing newline.
**Warning signs:** `done_received` is false for streams that otherwise completed normally.

### Pitfall 5: Panic in Extraction Breaking Client Stream

**What goes wrong:** A bug in JSON parsing or field extraction causes a panic inside the `.map()` closure. Without isolation, this unwinds through the stream machinery and terminates the client's connection.
**Why it happens:** `unwrap()` on unexpected JSON shapes, index out of bounds, or other logic errors.
**How to avoid:** Wrap the extraction call in `std::panic::catch_unwind(AssertUnwindSafe(|| ...))`. On `Err` (panic caught), log at error level and continue passing bytes through. The bytes are already the return value of the closure; extraction is a side effect.
**Warning signs:** Client streams terminating mid-response with connection reset.

### Pitfall 6: `data:` With and Without Space After Colon

**What goes wrong:** The SSE spec says "if value starts with a space, remove it." Most providers send `data: {...}` (with space), but some may send `data:{...}` (without space). Code that only checks `strip_prefix("data: ")` misses the no-space variant.
**Why it happens:** The spec allows both forms. OpenAI uses the space form, but other providers may not.
**How to avoid:** Check for both: `line.strip_prefix("data: ").or_else(|| line.strip_prefix("data:"))`.
**Warning signs:** Test fixtures only use one form; real providers use the other.

## Code Examples

### OpenAI SSE Stream Wire Format (Verified)

A typical streaming response with `stream_options: {"include_usage": true}`:

```
data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1693600000,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}],"usage":null}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1693600000,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":null}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1693600000,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":"stop"}],"usage":null}

data: {"id":"chatcmpl-abc123","object":"chat.completion.chunk","created":1693600060,"model":"gpt-4o","choices":[],"usage":{"prompt_tokens":6,"completion_tokens":10,"total_tokens":16}}

data: [DONE]

```

Key observations from OpenAI API docs and Phase 8 research:
- Each `data:` line is followed by two newlines (`\n\n`) -- one terminates the data field, one is the blank line event delimiter
- `usage` is `null` on all intermediate chunks
- The usage chunk has `choices: []` (empty array, not absent)
- `finish_reason` appears on the last content chunk (before the usage chunk), not on the usage chunk itself
- `data: [DONE]` is the final marker

Source: [OpenAI Developer Community](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156), [OpenAI API Reference](https://platform.openai.com/docs/api-reference/chat-streaming)

### SSE Line Endings (From WHATWG Spec)

The SSE specification defines three acceptable line endings:
- `\n` (LF) -- most common from Linux servers
- `\r\n` (CRLF) -- from Windows servers or proxies
- `\r` (CR alone) -- rare but spec-compliant

Per locked decision, we handle `\n` and `\r\n`. Standalone `\r` is rare enough to defer.

Source: [WHATWG HTML Living Standard - Server-Sent Events](https://html.spec.whatwg.org/multipage/server-sent-events.html)

### std::panic::catch_unwind in a Synchronous Closure

```rust
use std::panic::{catch_unwind, AssertUnwindSafe};

// Inside a .map() closure (synchronous):
let extraction_result = catch_unwind(AssertUnwindSafe(|| {
    observer.lock().unwrap().process_chunk(bytes);
}));

if let Err(panic_info) = extraction_result {
    tracing::error!(
        "Panic in SSE extraction (stream passthrough unaffected): {:?}",
        panic_info
    );
}
```

`catch_unwind` catches unwinding panics only (not `abort`). The `AssertUnwindSafe` wrapper is needed because the closure captures mutable state (the observer mutex). This is safe because:
1. If the panic occurred during `process_chunk`, the observer's state may be inconsistent, but that only affects extraction (usage/finish_reason), not the byte passthrough.
2. The Mutex is poisoned after a panic, but subsequent `lock().unwrap()` calls will also panic and be caught, so the observer degrades to "no extraction" rather than crashing.

Source: [Rust std docs - catch_unwind](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html)

### Recommended Test Fixture: Chunk Boundary Scenarios

```rust
/// Build SSE data from events, then split at the given byte positions.
fn split_sse_at_positions(events: &[&str], split_positions: &[usize]) -> Vec<Vec<u8>> {
    // Join events into a single byte buffer
    let full: Vec<u8> = events.iter()
        .flat_map(|e| format!("{}\n\n", e).into_bytes())
        .collect();

    // Split at the given byte positions
    let mut chunks = Vec::new();
    let mut prev = 0;
    for &pos in split_positions {
        if pos > prev && pos < full.len() {
            chunks.push(full[prev..pos].to_vec());
            prev = pos;
        }
    }
    chunks.push(full[prev..].to_vec());
    chunks
}

#[test]
fn usage_split_across_chunks() {
    let events = &[
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"},\"finish_reason\":\"stop\"}],\"usage\":null}",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}",
        "data: [DONE]",
    ];

    // Split in the middle of the usage JSON
    let chunks = split_sse_at_positions(events, &[50, 120, 180]);

    let mut observer = SseObserver::new(/* ... */);
    for chunk in &chunks {
        observer.process_chunk(chunk);
    }

    let result = observer.into_result();
    assert!(result.done_received);
    assert_eq!(result.usage, Some(StreamUsage { prompt_tokens: 10, completion_tokens: 5 }));
    assert_eq!(result.finish_reason, Some("stop".to_string()));
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Per-chunk `text.lines()` | Cross-chunk `Vec<u8>` buffer with line scanning | This phase | Correct handling of TCP chunk boundaries |
| No usage extraction from streams | Extract from final chunk's `usage` object | This phase (enabled by Phase 8's `stream_options` injection) | Streaming requests get accurate token counts |
| Full JSON parse of every chunk | Targeted extraction: only parse `data:` lines, skip known non-data SSE fields | This phase | Reduced per-chunk overhead |
| No panic isolation in stream closures | `catch_unwind` around extraction logic | This phase | Extraction bugs cannot crash client streams |

**Deprecated/outdated:**
- The existing extraction code in `handle_streaming_response` (handlers.rs:680-707) parses chunks but has no cross-chunk buffer and no mechanism to communicate extracted data back to the handler. Phase 9 replaces this with the `SseObserver` approach.

## Open Questions

1. **Poisoned Mutex After Panic**
   - What we know: If `process_chunk` panics inside the `catch_unwind`, the `Arc<Mutex<SseObserver>>` becomes poisoned. Subsequent `lock().unwrap()` calls on later chunks will also panic (caught by `catch_unwind`).
   - What's unclear: Should the observer detect a poisoned mutex and short-circuit (skip extraction entirely after first panic)?
   - Recommendation: Use `lock().unwrap_or_else(|e| e.into_inner())` to recover from poisoned mutexes. This accesses the inner data despite the poison, which is acceptable because the observer's purpose is best-effort extraction -- inconsistent state is better than no extraction at all.

2. **Maximum Buffer Size**
   - What we know: A normal SSE data line is ~200-500 bytes. A pathological case (malformed provider sending no newlines) could grow the buffer indefinitely.
   - What's unclear: What is a reasonable maximum? The PITFALLS.md suggests 64KB.
   - Recommendation: Cap the buffer at 64KB. If exceeded, drain the buffer entirely (losing the partial line) and log a warning. This prevents OOM while accepting one lost line in a pathological case. This is at Claude's discretion per the decisions.

3. **Drop Timing and Result Availability**
   - What we know: The `Drop` impl on `SseObserver` writes the result to the shared handle. Phase 10 reads the handle after the stream is consumed.
   - What's unclear: When exactly does the stream drop relative to when Phase 10 can read the result?
   - Recommendation: The stream is consumed by axum's response writer. After the response is fully sent, the stream (and observer) are dropped. Phase 10 needs to detect stream completion -- this is Phase 10's concern, not Phase 9's. Phase 9 guarantees that when the observer is dropped, the result is available in the handle.

## Sources

### Primary (HIGH confidence)
- **Codebase inspection:** `src/proxy/handlers.rs` (existing stream adapter at lines 674-716, per-chunk parsing pattern), `src/proxy/types.rs` (ChatCompletionChunk without usage field, StreamOptions struct), `src/storage/logging.rs` (update_usage function from Phase 8)
- **PITFALLS.md:** `.planning/research/PITFALLS.md` -- comprehensive pitfall analysis for SSE stream interception, TCP chunk splitting, tee architecture, buffer management
- **Phase 8 Research:** `.planning/phases/08-stream-request-foundation/08-RESEARCH.md` -- OpenAI SSE format with stream_options, final usage chunk structure, verified format

### Secondary (MEDIUM confidence)
- [WHATWG SSE Specification](https://html.spec.whatwg.org/multipage/server-sent-events.html) -- line ending formats, field types, space-after-colon rule
- [OpenAI Developer Community](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156) -- stream_options, final usage chunk format
- [OpenRouter Streaming Docs](https://openrouter.ai/docs/api/reference/streaming) -- SSE comments, keepalive, `finish_reason: "error"` pattern
- [Rust std docs - catch_unwind](https://doc.rust-lang.org/std/panic/fn.catch_unwind.html) -- synchronous panic catching, UnwindSafe requirement
- [futures StreamExt docs](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html) -- map(), scan(), catch_unwind(), inspect() combinators
- [Tokio framing tutorial](https://tokio.rs/tokio/tutorial/framing) -- line buffering for byte streams

### Tertiary (LOW confidence)
- None. All findings verified against codebase, official specifications, or library documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies needed, all patterns exist in codebase and `futures` crate
- Architecture: HIGH -- extends the existing `.map()` closure pattern with a buffer; PITFALLS.md covers all edge cases; `Drop`-based finalization is a proven Rust pattern
- Pitfalls: HIGH -- PITFALLS.md has comprehensive coverage; TCP chunk splitting, non-UTF8 bytes, and `[DONE]` handling are well-documented concerns
- Testing: HIGH -- chunk boundary tests are straightforward with the `split_sse_at_positions` helper; no external services needed

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable -- SSE spec is frozen, OpenAI streaming format is established, Rust stream patterns are mature)
