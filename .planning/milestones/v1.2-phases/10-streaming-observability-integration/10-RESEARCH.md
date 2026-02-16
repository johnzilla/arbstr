# Phase 10: Streaming Observability Integration - Research

**Researched:** 2026-02-16
**Domain:** SSE stream proxying with post-stream observability (axum + tokio + futures)
**Confidence:** HIGH

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

#### Trailing SSE event format
- Plain `data:` line (no `event:` field) -- same format as all other SSE chunks, distinguished by payload content
- Positioned after the upstream `[DONE]` passes through, followed by arbstr's own `data: [DONE]`
- JSON structure nested under key: `{"arbstr": {"cost_sats": 42, "latency_ms": 1200}}`
- Fields: cost_sats and latency_ms only -- matches success criteria, minimal payload

#### Completion status
- Reuse existing `success` BOOLEAN + `error_message` TEXT columns -- no schema change for status
- Client disconnection detected via stream send error (broken pipe / connection reset)
- On client disconnect, continue consuming upstream to extract usage data for DB update
- Provider errors during streaming treated same as pre-stream errors -- no differentiation needed

#### Degradation behavior
- Always emit trailing SSE event, even when provider sends no usage data -- include latency, null cost
- When usage present but cost can't be calculated (no rate configured), use `null` for cost_sats (not zero)
- Always update DB with token/cost data if extracted, regardless of client connection status
- On SseObserver panic (caught by catch_unwind), still emit trailing event with available data (latency always available, null cost)

#### Latency boundaries
- Timer starts at request send time (when arbstr sends to upstream provider) -- full round-trip including network
- Timer ends at last upstream byte received (before arbstr's trailing event) -- measures pure provider time
- Keep both TTFB and full duration: existing `latency_ms` stays as TTFB from INSERT, add `stream_duration_ms` for full stream duration via UPDATE
- On client disconnect, latency still measures full upstream duration (not capped at disconnect time)

### Claude's Discretion
- How to wire wrap_sse_stream into the existing handler streaming path
- Error message text for different failure modes
- Trailing event serialization implementation details
- New migration for stream_duration_ms column

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

## Summary

Phase 10 wires two independently-built subsystems into the request handler: Phase 8's `stream_options` injection + `spawn_usage_update` and Phase 9's `wrap_sse_stream` + `StreamResultHandle`. The core challenge is replacing the current passthrough-only `handle_streaming_response` function with a new pipeline that (1) wraps the upstream byte stream for observation, (2) proxies all bytes to the client in real time, (3) after the upstream completes, reads the extracted `StreamResult`, computes cost, emits a trailing SSE event to the client, and (4) fires off a database UPDATE with tokens, cost, latency, and completion status.

The key architectural decision is how to build the axum response body. The current code uses `Body::from_stream(upstream.bytes_stream().map(...))`, a simple passthrough. Phase 10 must replace this with a **tokio mpsc channel-based body** where a spawned background task consumes the upstream stream (through `wrap_sse_stream` for observation), forwards each chunk to the channel (detecting client disconnect on send failure), and after the upstream ends, reads the `StreamResultHandle`, computes the trailing event, sends it to the channel, then performs the DB update. This approach naturally handles client disconnect (channel send returns `Err` when receiver/body is dropped), allows the background task to continue consuming upstream even after disconnect, and cleanly separates the concerns.

**Primary recommendation:** Use a `tokio::sync::mpsc` channel as the response body (via `Body::from_stream(ReceiverStream::new(rx))`), with a `tokio::spawn`ed task that consumes the `wrap_sse_stream` output, forwards bytes, appends the trailing event, and fires the DB update.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio | 1.x | mpsc channel for body, spawn for background task | Already in project, standard async runtime |
| futures | 0.3 | StreamExt for consuming wrapped stream | Already in project, standard stream utilities |
| axum | 0.7 | Body::from_stream for channel-based body | Already in project |
| sqlx | 0.8 | SQLite migration for stream_duration_ms | Already in project |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tokio-stream | 0.1 | ReceiverStream wrapper for mpsc Receiver | Convert mpsc::Receiver into Stream for Body::from_stream |
| serde_json | 1 | Serialize trailing SSE event JSON | Already in project |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| mpsc channel body | futures::stream::chain | chain cannot produce trailing event that depends on observing the entire first stream; it requires the appended stream to be known upfront |
| mpsc channel body | async_stream try_stream! | Would add a new dependency; mpsc is already available and simpler to reason about for this pattern |
| tokio::spawn background task | Inline stream combinator | Cannot continue consuming upstream after client disconnect; cannot perform async DB writes after stream ends |

**Additional dependency:**
```bash
cargo add tokio-stream
```
Note: `tokio-stream` provides `ReceiverStream` to convert `tokio::sync::mpsc::Receiver` into a `Stream`. This is a lightweight crate from the tokio project, commonly paired with axum for channel-based streaming bodies.

## Architecture Patterns

### Recommended Architecture: Channel-Based Streaming Proxy

The handler returns a response body backed by an mpsc channel. A spawned task owns the upstream consumption loop.

```
Client <--- mpsc channel (Body) <--- spawned task ---> upstream provider
                                          |
                                          +--> wrap_sse_stream (observation)
                                          +--> StreamResultHandle (usage/finish)
                                          +--> trailing SSE event
                                          +--> DB UPDATE (fire-and-forget)
```

### Pattern 1: handle_streaming_response Restructure

**What:** Replace the current passthrough stream in `handle_streaming_response` with a channel-based body and a spawned background task.

**Current code flow (to be replaced):**
```rust
// Current: simple passthrough, no observation, no trailing event
let stream = upstream_response.bytes_stream().map(move |chunk| {
    // logging only, no extraction
    chunk.map_err(std::io::Error::other)
});
let body = Body::from_stream(stream);
```

**New code flow:**
```rust
// Phase 10: channel-based body with background task
fn handle_streaming_response(
    upstream_response: reqwest::Response,
    provider: &SelectedProvider,
    correlation_id: String,
    pool: Option<SqlitePool>,
    stream_start: std::time::Instant,
) -> Result<RequestOutcome, RequestError> {
    let provider_name = provider.name.clone();
    let input_rate = provider.input_rate;
    let output_rate = provider.output_rate;
    let base_fee = provider.base_fee;

    // Create channel for body
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

    // Wrap upstream stream for SSE observation
    let (observed_stream, result_handle) = wrap_sse_stream(upstream_response.bytes_stream());

    // Spawn background task: consume upstream, forward to channel, emit trailing, DB update
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut client_connected = true;
        futures::pin_mut!(observed_stream);

        // Forward all upstream chunks to client
        while let Some(chunk_result) = observed_stream.next().await {
            if client_connected {
                match &chunk_result {
                    Ok(bytes) => {
                        if tx.send(Ok(bytes.clone())).await.is_err() {
                            // Client disconnected -- continue consuming upstream
                            client_connected = false;
                            tracing::info!("Client disconnected during stream");
                        }
                    }
                    Err(_) => {
                        // Upstream error -- forward to client if connected
                        if tx.send(chunk_result).await.is_err() {
                            client_connected = false;
                        }
                    }
                }
            }
            // If client disconnected, we still consume upstream (for usage extraction)
        }

        // Stream complete -- measure duration
        let stream_duration_ms = stream_start.elapsed().as_millis() as i64;

        // Read extracted result from observer
        let stream_result = result_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();

        // Compute cost and build trailing event
        let (input_tokens, output_tokens, cost_sats) = match &stream_result {
            Some(sr) => match &sr.usage {
                Some(usage) => {
                    let cost = actual_cost_sats(
                        usage.prompt_tokens, usage.completion_tokens,
                        input_rate, output_rate, base_fee,
                    );
                    (Some(usage.prompt_tokens), Some(usage.completion_tokens), Some(cost))
                }
                None => (None, None, None),
            },
            None => (None, None, None),
        };

        // Emit trailing SSE event (if client still connected)
        if client_connected {
            let trailing = build_trailing_sse_event(cost_sats, stream_duration_ms);
            let _ = tx.send(Ok(Bytes::from(trailing))).await;
        }

        // DB update (fire-and-forget, always, regardless of client status)
        if let Some(pool) = pool {
            // ... spawn_usage_update + stream_duration update
        }
    });

    // Build response with channel-backed body
    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .header(CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap();

    Ok(RequestOutcome {
        response,
        provider_name,
        input_tokens: None,  // Will be updated via DB UPDATE
        output_tokens: None,
        cost_sats: None,
        provider_cost_sats: None,
    })
}
```

### Pattern 2: Trailing SSE Event Builder

**What:** Serialize the trailing event in the locked-in format.

```rust
fn build_trailing_sse_event(cost_sats: Option<f64>, latency_ms: i64) -> Vec<u8> {
    let cost_value = match cost_sats {
        Some(c) => serde_json::Value::Number(
            serde_json::Number::from_f64(c).unwrap_or(serde_json::Number::from(0))
        ),
        None => serde_json::Value::Null,
    };

    let event_json = serde_json::json!({
        "arbstr": {
            "cost_sats": cost_value,
            "latency_ms": latency_ms
        }
    });

    // Format: data: {json}\n\ndata: [DONE]\n\n
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&event_json).unwrap()
    ).into_bytes()
}
```

**Key:** The trailing event comes AFTER the upstream `[DONE]` passes through (which `wrap_sse_stream` forwards unmodified), followed by arbstr's own `data: [DONE]`.

### Pattern 3: DB Update After Stream Completion

**What:** Extended `update_usage` that also writes `stream_duration_ms` and updates `success`/`error_message` for completion status.

```rust
// New function or extended update_usage
pub async fn update_stream_completion(
    pool: &SqlitePool,
    correlation_id: &str,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
    stream_duration_ms: i64,
    success: bool,
    error_message: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE requests SET
            input_tokens = ?, output_tokens = ?, cost_sats = ?,
            stream_duration_ms = ?, success = ?, error_message = ?
         WHERE correlation_id = ?"
    )
    .bind(input_tokens.map(|v| v as i64))
    .bind(output_tokens.map(|v| v as i64))
    .bind(cost_sats)
    .bind(stream_duration_ms)
    .bind(success)
    .bind(error_message)
    .bind(correlation_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
```

### Pattern 4: Handler Signature Change

**What:** The streaming path in `chat_completions` needs to pass additional context to `handle_streaming_response`.

The current `handle_streaming_response` only receives the upstream response and provider. Phase 10 needs it to also receive:
- `correlation_id: String` (for DB update)
- `pool: Option<SqlitePool>` (for DB update)
- `stream_start: std::time::Instant` (for latency measurement)

The handler's initial DB INSERT (for TTFB latency) stays as-is. The spawned task's DB UPDATE fills in the token/cost/duration/status fields afterward.

### Pattern 5: Migration for stream_duration_ms

```sql
-- migrations/YYYYMMDDHHMMSS_add_stream_duration.sql
ALTER TABLE requests ADD COLUMN stream_duration_ms INTEGER;
```

SQLite `ALTER TABLE ADD COLUMN` supports nullable columns with no default. This column is NULL for non-streaming requests and old rows, which is semantically correct.

### Anti-Patterns to Avoid

- **Trying to use `stream.chain()` for trailing event:** The trailing event content depends on the full upstream stream's `StreamResult`, which is only known after the first stream ends. `chain` requires the second stream to be constructable before consuming the first. This doesn't work.
- **Blocking on StreamResultHandle inside a stream combinator:** Using `.lock()` in a `futures::stream::map` closure risks deadlock since the observer might be writing to the handle from its Drop impl at the same time.
- **Computing cost inside the stream combinator:** Cost requires the full usage data which is only available after all chunks are consumed. Must be done in the post-stream phase.
- **Dropping upstream on client disconnect:** The user decision requires continuing to consume upstream for usage extraction even after client disconnects. The background task must keep consuming.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| mpsc Receiver to Stream | Custom poll-based Stream impl | tokio_stream::wrappers::ReceiverStream | Handles wakeup and backpressure correctly |
| SSE observation | New extraction logic | wrap_sse_stream from Phase 9 | Already built with panic isolation, cross-chunk handling, Drop finalization |
| stream_options injection | Manual JSON manipulation | ensure_stream_options from Phase 8 | Already handles merge semantics correctly |
| DB token update | Raw SQL in handler | spawn_usage_update / new spawn_stream_completion | Follows established fire-and-forget pattern |

**Key insight:** Phase 10 is primarily an integration phase. All the hard parsing/extraction/injection logic already exists. The new code is the orchestration in the handler and the channel-based body pattern.

## Common Pitfalls

### Pitfall 1: Dropping the Sender Before Stream Completes
**What goes wrong:** If the spawned task drops the `tx` sender (by returning or panicking) before sending all data, the client receives an incomplete response that may not even be valid SSE.
**Why it happens:** Early return on error without completing the trailing event.
**How to avoid:** Wrap the entire task body in a structure that ensures trailing event + cleanup always runs. Use a `defer`-style pattern or explicit error handling at each step.
**Warning signs:** Clients receiving truncated SSE without `[DONE]`.

### Pitfall 2: Channel Buffer Size Too Small
**What goes wrong:** With a very small channel buffer, the spawned task blocks waiting to send, adding latency to every chunk. With too large, memory usage spikes on slow clients.
**Why it happens:** mpsc channel backpressure.
**How to avoid:** Use a reasonable buffer size (32 is standard for streaming proxies). This allows bursts without excessive memory.
**Warning signs:** Increased TTFB or chunk latency compared to current passthrough.

### Pitfall 3: Clone of Bytes on Every Chunk
**What goes wrong:** `Bytes::clone()` is cheap (reference-counted, O(1)), but if you accidentally clone the underlying `Vec<u8>` instead, every chunk incurs a full copy.
**Why it happens:** Confusing `bytes::Bytes` (cheap clone via Arc) with `Vec<u8>` (expensive clone).
**How to avoid:** Always work with `bytes::Bytes` in the forwarding path. The `wrap_sse_stream` output already yields `Bytes`.
**Warning signs:** High CPU usage during streaming.

### Pitfall 4: DB INSERT and UPDATE Race Condition
**What goes wrong:** The spawned task's UPDATE runs before the handler's INSERT completes, resulting in zero rows affected.
**Why it happens:** Both are fire-and-forget spawns. The INSERT is spawned from the handler path; the UPDATE from the background task. The stream might complete very quickly (e.g., short response).
**How to avoid:** The INSERT happens synchronously in the handler path BEFORE the response is returned to the client. Since the response body starts streaming only after the handler returns, and the background task only starts consuming after the stream begins flowing, the INSERT will always complete before the UPDATE. However, to be safe, add a small yield or verify the INSERT uses `spawn_log_write` which runs as a separate tokio task. The timing should be safe because: (1) handler creates INSERT task, (2) handler creates response with channel body, (3) handler returns response, (4) axum starts polling body stream, (5) background task starts consuming upstream and forwarding, (6) eventually background task finishes and runs UPDATE. Step 1 happens before step 5.
**Warning signs:** `update_usage affected zero rows` warning in logs.

### Pitfall 5: f64 NaN in Trailing Event JSON
**What goes wrong:** `serde_json::Number::from_f64(NaN)` returns `None`, producing an unexpected null or panic.
**Why it happens:** Division by zero or overflow in cost calculation.
**How to avoid:** `actual_cost_sats` uses multiplication and division by 1000 with unsigned integer rates cast to f64 -- NaN is not possible in normal operation. But guard with `from_f64(...).unwrap_or(...)` just in case.
**Warning signs:** Trailing event has unexpected null cost when usage was present.

### Pitfall 6: Forgetting to Send arbstr's Own [DONE]
**What goes wrong:** Client SSE parsers wait forever for stream termination if the final `[DONE]` is not sent.
**Why it happens:** The upstream `[DONE]` passes through via `wrap_sse_stream`. After that, arbstr emits its trailing data event. It MUST also emit its own `data: [DONE]` to terminate the stream.
**How to avoid:** The `build_trailing_sse_event` function must include `data: [DONE]\n\n` after the arbstr metadata line.
**Warning signs:** Client hangs after receiving arbstr metadata.

## Code Examples

### Trailing SSE Event Wire Format

Per the locked decision, the complete trailing sequence looks like:

```
... (upstream chunks pass through) ...
data: [DONE]          <-- upstream's DONE, forwarded by wrap_sse_stream
                       <-- empty line (SSE event delimiter, part of upstream)
data: {"arbstr":{"cost_sats":42.35,"latency_ms":1200}}
                       <-- empty line
data: [DONE]          <-- arbstr's DONE
                       <-- empty line
```

As raw bytes:
```rust
// Source: Locked decision in CONTEXT.md
let trailing = format!(
    "data: {}\n\ndata: [DONE]\n\n",
    serde_json::to_string(&serde_json::json!({
        "arbstr": {
            "cost_sats": cost_sats,  // f64 or null
            "latency_ms": stream_duration_ms
        }
    })).unwrap()
);
```

### ReceiverStream as Body

```rust
// Source: tokio-stream docs + axum Body::from_stream
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use axum::body::Body;
use bytes::Bytes;

let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(32);
let body = Body::from_stream(ReceiverStream::new(rx));
// tx.send(Ok(Bytes::from(...))).await -- in spawned task
```

### Client Disconnect Detection

```rust
// Source: tokio mpsc semantics
// When the Body (and thus the ReceiverStream/Receiver) is dropped by axum
// because the client disconnected, tx.send() returns Err.
if tx.send(Ok(bytes)).await.is_err() {
    // Client disconnected. Stop sending but keep consuming upstream.
    client_connected = false;
    tracing::info!(correlation_id = %cid, "Client disconnected during stream");
}
```

### Completion Status Mapping

```rust
// Source: CONTEXT.md locked decisions
// Determine success/error_message based on stream outcome
let (success, error_message) = if let Some(ref sr) = stream_result {
    if sr.done_received {
        if client_connected {
            (true, None) // Normal completion
        } else {
            (true, Some("client_disconnected".to_string()))
            // success=true because upstream completed normally
        }
    } else {
        (false, Some("stream_incomplete".to_string()))
    }
} else {
    (false, Some("no_stream_result".to_string()))
};
```

### Migration File

```sql
-- Source: SQLite ALTER TABLE documentation
-- File: migrations/YYYYMMDDHHMMSS_add_stream_duration.sql
ALTER TABLE requests ADD COLUMN stream_duration_ms INTEGER;
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Passthrough stream (current) | Channel-based body with background task | Phase 10 | Enables post-stream actions (trailing event, DB update) |
| TTFB only latency | TTFB + stream_duration_ms | Phase 10 | Full stream timing visible in DB |
| No streaming usage data | Usage extracted via SseObserver | Phase 9 | Tokens/cost available for DB update |
| No stream_options injection | Automatic include_usage injection | Phase 8 | Providers send usage in final chunk |

## Open Questions

1. **tokio-stream dependency**
   - What we know: `ReceiverStream` is the standard way to convert a tokio mpsc Receiver into a futures Stream. The crate is lightweight and maintained by the tokio team.
   - What's unclear: Whether the project wants to add this dependency or use an alternative (e.g., `futures::stream::unfold` polling the receiver manually).
   - Recommendation: Add `tokio-stream` -- it is the canonical approach and avoids hand-rolling a poll-based wrapper. It is already a transitive dependency of several crates in the tree.

2. **Channel buffer size**
   - What we know: Buffer size affects backpressure behavior. Too small adds latency, too large wastes memory.
   - What's unclear: Optimal size for LLM streaming responses (which arrive in small SSE chunks, ~100-500 bytes each).
   - Recommendation: Use 32 as initial value. This allows burst absorption while keeping memory under 16KB per stream. Can be tuned later.

3. **Existing `latency_ms` INSERT behavior for streaming**
   - What we know: Currently, the handler logs `latency_ms` at INSERT time (TTFB -- time from request start to when the response is returned to axum). Per locked decision, this stays as TTFB, with `stream_duration_ms` added via UPDATE.
   - What's unclear: The current streaming handler measures latency BEFORE the response body starts streaming. This is actually measuring "time to get HTTP 200 from upstream" which is close to TTFB but includes arbstr processing. This is acceptable per the decision to keep both measurements.
   - Recommendation: No change to existing INSERT latency. The new `stream_duration_ms` UPDATE measures wall-clock time from request send to last upstream byte.

## Sources

### Primary (HIGH confidence)
- Codebase analysis of `src/proxy/handlers.rs` -- current streaming handler structure, RequestOutcome pattern, attach_arbstr_headers
- Codebase analysis of `src/proxy/stream.rs` -- wrap_sse_stream API signature, StreamResultHandle type, StreamResult fields
- Codebase analysis of `src/storage/logging.rs` -- spawn_usage_update pattern, update_usage SQL, RequestLog struct
- Codebase analysis of `src/proxy/types.rs` -- ensure_stream_options, ChatCompletionRequest
- Codebase analysis of Phase 8 and Phase 9 summaries -- confirmed what was built and public APIs

### Secondary (MEDIUM confidence)
- [futures StreamExt docs](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html) -- chain, map, next semantics
- [tokio mpsc channel docs](https://docs.rs/tokio/latest/tokio/sync/mpsc/) -- send semantics, Err on closed receiver
- [axum discussions on SSE disconnect](https://github.com/tokio-rs/axum/discussions/3151) -- confirmed channel send error as disconnect signal
- [sqlx migrate! macro docs](https://docs.rs/sqlx/latest/sqlx/macro.migrate.html) -- embedded migration workflow

### Tertiary (LOW confidence)
- [async_stream try_stream! docs](https://docs.rs/async-stream/latest/async_stream/macro.try_stream.html) -- alternative approach, not recommended
- [tokio-stream ReceiverStream](https://docs.rs/tokio-stream/) -- standard wrapper, not verified against latest version

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- all libraries already in project or canonical tokio ecosystem
- Architecture: HIGH -- channel-based streaming proxy is well-established pattern, codebase structure is fully understood
- Pitfalls: HIGH -- identified from direct code analysis of race conditions, disconnect handling, and SSE formatting
- Wiring approach: HIGH -- based on thorough reading of all relevant source files and Phase 8/9 outputs

**Research date:** 2026-02-16
**Valid until:** 2026-03-16 (stable domain, all libraries are mature)
