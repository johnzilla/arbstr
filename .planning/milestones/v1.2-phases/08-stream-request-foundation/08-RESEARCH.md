# Phase 8: Stream Request Foundation - Research

**Researched:** 2026-02-15
**Domain:** OpenAI streaming API (stream_options injection), SQLite UPDATE queries, serde JSON manipulation
**Confidence:** HIGH

## Summary

Phase 8 has two distinct deliverables: (1) injecting `stream_options: {"include_usage": true}` into upstream streaming requests so providers return token usage data in the final SSE chunk, and (2) adding a database UPDATE function that writes post-stream token counts and cost to an existing request log row.

Both are well-understood problems with clear implementation paths in the existing codebase. The request injection requires adding a `StreamOptions` struct and a `stream_options` field to `ChatCompletionRequest`, plus an injection function called at send time. The database UPDATE requires a new function in `storage/logging.rs` that matches the existing `spawn_log_write` pattern. No new dependencies are needed -- the current stack (serde, sqlx, tokio) handles everything.

**Primary recommendation:** Add `stream_options` as an `Option<StreamOptions>` field on `ChatCompletionRequest` with a standalone injection function that merges rather than overwrites, called in `send_to_provider` before serialization. Add an `update_usage` method to `RequestLog` (or a standalone function) plus a `spawn_usage_update` fire-and-forget wrapper matching the existing INSERT pattern.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Request Mutation:**
- Merge with client values: if client already sends `stream_options`, preserve their settings and only add `include_usage: true` if missing (don't override)
- Always inject for streaming requests unconditionally -- not gated on logging or mock mode
- Only inject when `stream: true` -- non-streaming requests left completely untouched
- Injection happens at send time (when building reqwest body), not earlier in the handler

**DB Update Strategy:**
- Await INSERT completion before starting stream to prevent race condition (UPDATE must find the row)
- UPDATE writes `input_tokens`, `output_tokens`, `cost_sats` only -- latency stays as TTFB from INSERT (Phase 10 handles full-stream latency)
- Fire-and-forget pattern (tokio::spawn, warn on failure) -- consistent with existing INSERT
- Warn via tracing if UPDATE affects zero rows (rows_affected == 0) -- indicates something went wrong

**Provider Compatibility:**
- Universal injection -- no per-provider config toggle. If a provider rejects stream_options, existing retry/fallback handles the error
- If no usage data extracted from stream, still run UPDATE with NULLs to mark stream completed
- Debug-level log when provider didn't return usage data -- only visible with RUST_LOG=debug
- Real provider testing deferred to Phase 10 integration -- Phase 8 uses mock providers only

**Testing Approach:**
- Both unit tests (injection function) AND integration tests (HTTP call to mock server)
- Integration test: mock provider captures and inspects request body, asserts stream_options present in serialized JSON
- DB UPDATE tested with in-memory SQLite: INSERT row, run UPDATE, verify columns changed
- Full test suite (cargo test) must pass with zero failures before phase is done

### Claude's Discretion
- Exact placement of injection function (standalone fn vs method on request type)
- StreamOptions struct design (fields, serde attributes)
- UPDATE query construction and column selection
- Test organization (new test file vs extend existing)

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope
</user_constraints>

## Standard Stack

### Core (already in Cargo.toml -- no additions needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| serde | 1 | Serialize/deserialize `StreamOptions` struct | Already used for all types in `proxy/types.rs` |
| serde_json | 1 | JSON manipulation if needed for merge logic | Already a dependency |
| sqlx | 0.8 | SQLite UPDATE query with `rows_affected()` | Already used for INSERT in `storage/logging.rs` |
| tokio | 1 | `tokio::spawn` for fire-and-forget UPDATE | Already used for INSERT pattern |
| tracing | 0.1 | `warn!` on zero rows, `debug!` for no-usage | Already used everywhere |
| wiremock | 0.6 | Integration test mock server for request inspection | Already in dev-dependencies |

### Supporting (no new additions)

No new libraries are required. The existing stack fully covers this phase.

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Adding `stream_options` field to struct | Serialize to `serde_json::Value` and inject dynamically | Struct field is cleaner, type-safe, and consistent with existing pattern; Value manipulation is error-prone |
| `Option<StreamOptions>` field | `#[serde(flatten)] HashMap<String, Value>` for extra fields | Flatten approach captures ALL unknown fields but adds complexity; we only need one known field |

## Architecture Patterns

### Recommended Changes (File-by-File)

```
src/
├── proxy/
│   ├── types.rs      # ADD: StreamOptions struct, stream_options field on ChatCompletionRequest
│   └── handlers.rs   # MODIFY: call inject function in send_to_provider before .json()
└── storage/
    └── logging.rs    # ADD: update_usage() method/fn and spawn_usage_update() wrapper
```

### Pattern 1: StreamOptions Struct Design

**What:** A new struct representing the `stream_options` object in the OpenAI API.
**When to use:** Serialized into the request body when `stream: true`.

```rust
/// Options for streaming responses (OpenAI-compatible).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamOptions {
    /// When true, the final streaming chunk includes a usage object
    /// with prompt_tokens and completion_tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
    // Future: include_obfuscation and other stream options
}
```

The field on `ChatCompletionRequest`:
```rust
pub struct ChatCompletionRequest {
    // ... existing fields ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}
```

**Key serde attributes:**
- `skip_serializing_if = "Option::is_none"` on the field ensures non-streaming requests never emit `stream_options` in the JSON (backward compatible).
- `Option<bool>` for `include_usage` rather than bare `bool` -- this allows distinguishing "not set" from "set to false", supporting the merge-not-override decision.

### Pattern 2: Injection at Send Time (Merge Strategy)

**What:** A function that ensures `stream_options.include_usage` is `true` on the request, merging with any client-provided values.
**When to use:** Called in `send_to_provider` only when `is_streaming` is true, immediately before serialization.

```rust
/// Ensure stream_options.include_usage is true for streaming requests.
///
/// Merges with any existing client-provided stream_options rather than
/// overriding. Only adds include_usage if not already set.
fn ensure_stream_options(request: &mut ChatCompletionRequest) {
    match &mut request.stream_options {
        Some(opts) => {
            // Preserve existing settings, only add include_usage if missing
            if opts.include_usage.is_none() {
                opts.include_usage = Some(true);
            }
        }
        None => {
            request.stream_options = Some(StreamOptions {
                include_usage: Some(true),
            });
        }
    }
}
```

**Integration point in `send_to_provider`:**
The current code at line 492-497 of `handlers.rs`:
```rust
let mut upstream_request = state
    .http_client
    .post(&upstream_url)
    .header(header::CONTENT_TYPE, "application/json")
    .header("Idempotency-Key", correlation_id)
    .json(request);
```

This needs to change to clone and mutate the request before serialization:
```rust
// Inject stream_options for streaming requests
let body = if is_streaming {
    let mut req = request.clone();
    ensure_stream_options(&mut req);
    req
} else {
    request.clone()
};

let mut upstream_request = state
    .http_client
    .post(&upstream_url)
    .header(header::CONTENT_TYPE, "application/json")
    .header("Idempotency-Key", correlation_id)
    .json(&body);
```

**Note:** `request` is already `&ChatCompletionRequest` (borrowed), so we must clone to mutate. The clone cost is negligible compared to the HTTP round-trip.

### Pattern 3: Fire-and-Forget Database UPDATE

**What:** An UPDATE function matching the existing `spawn_log_write` pattern.
**When to use:** Called after stream completes to write extracted usage data back to the row.

```rust
/// Update token counts and cost on an existing request log entry.
///
/// Matches by correlation_id. Returns the number of rows affected.
pub async fn update_usage(
    pool: &SqlitePool,
    correlation_id: &str,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE requests SET input_tokens = ?, output_tokens = ?, cost_sats = ?
         WHERE correlation_id = ?"
    )
    .bind(input_tokens.map(|v| v as i64))
    .bind(output_tokens.map(|v| v as i64))
    .bind(cost_sats)
    .bind(correlation_id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

/// Spawn a fire-and-forget usage update.
///
/// Warns if the UPDATE affects zero rows (row not found) or if the
/// query itself fails.
pub fn spawn_usage_update(
    pool: &SqlitePool,
    correlation_id: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
) {
    let pool = pool.clone();
    tokio::spawn(async move {
        match update_usage(&pool, &correlation_id, input_tokens, output_tokens, cost_sats).await {
            Ok(0) => {
                tracing::warn!(
                    correlation_id = %correlation_id,
                    "Usage update affected zero rows -- request log entry not found"
                );
            }
            Ok(_) => {
                tracing::debug!(
                    correlation_id = %correlation_id,
                    input_tokens = ?input_tokens,
                    output_tokens = ?output_tokens,
                    cost_sats = ?cost_sats,
                    "Updated request log with usage data"
                );
            }
            Err(e) => {
                tracing::warn!(
                    correlation_id = %correlation_id,
                    error = %e,
                    "Failed to update request log with usage data"
                );
            }
        }
    });
}
```

### Pattern 4: Awaiting INSERT Before Streaming (Race Prevention)

**What:** The streaming path must await the INSERT before returning the response to prevent the UPDATE racing.
**When to use:** The current streaming handler fires INSERT via `spawn_log_write` (fire-and-forget). For Phase 8, this INSERT must complete before the stream begins.

**Current code (lines 166-203):**
```rust
if let Some(pool) = &state.db {
    let log_entry = match &result { /* ... */ };
    spawn_log_write(pool, log_entry);  // fire-and-forget
}
```

**Changed pattern:**
```rust
if let Some(pool) = &state.db {
    let log_entry = match &result { /* ... */ };
    // Await INSERT completion so the row exists before any UPDATE
    if let Err(e) = log_entry.insert(pool).await {
        tracing::warn!(
            correlation_id = %correlation_id,
            error = %e,
            "Failed to write request log to database"
        );
    }
}
```

**Important:** This changes the INSERT from fire-and-forget to awaited ONLY for streaming requests. Non-streaming requests continue using `spawn_log_write` unchanged. The INSERT is fast (local SQLite WAL mode, typically <1ms) so this does not meaningfully delay the response.

### Anti-Patterns to Avoid

- **Modifying the original request reference:** The `request` parameter in `send_to_provider` is `&ChatCompletionRequest`. Clone before mutating -- do not change the signature to `&mut`.
- **Injecting stream_options in the handler:** The decision is to inject at send time in `send_to_provider`, not earlier. This keeps the mutation close to serialization and avoids affecting retry logic.
- **Using `serde_json::Value` for injection:** While technically possible (serialize to Value, insert key, serialize back to string), this loses type safety and is fragile. The struct-based approach is correct here.
- **Writing a new migration for the UPDATE:** The existing schema already has nullable `input_tokens`, `output_tokens`, and `cost_sats` columns. No schema change needed.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| JSON field injection | Manual string manipulation of JSON | Struct field with serde serialization | String manipulation is error-prone with escaping |
| Async fire-and-forget | Manual thread spawn | `tokio::spawn` matching existing pattern | Consistency with `spawn_log_write` |
| Database connection management | Manual connection lifecycle | Existing `SqlitePool` from app state | Pool already handles WAL mode, connection reuse |

**Key insight:** Every piece of infrastructure needed already exists in the codebase. This phase adds new functions that compose existing patterns -- no new infrastructure is required.

## Common Pitfalls

### Pitfall 1: Race Condition Between INSERT and UPDATE

**What goes wrong:** If INSERT is fire-and-forget (tokio::spawn) and the stream completes quickly, the UPDATE may execute before the INSERT, finding zero rows.
**Why it happens:** The INSERT is spawned on a separate task and may not complete before the stream starts flowing and the usage chunk arrives.
**How to avoid:** Await the INSERT for streaming requests (as specified in locked decisions). The INSERT is local SQLite (~1ms) so the delay is negligible.
**Warning signs:** `rows_affected == 0` warnings in logs with correlation IDs that should exist.

### Pitfall 2: Overriding Client stream_options

**What goes wrong:** Client sends `stream_options: {"include_usage": false}` and the injection blindly replaces the entire object, losing other client settings.
**Why it happens:** Using assignment (`request.stream_options = Some(...)`) instead of merge logic.
**How to avoid:** The merge function checks for existing `stream_options` and only adds `include_usage` if not already present. If the client explicitly sets `include_usage: false`, we do NOT override it per the locked decision ("only add include_usage: true if missing").
**Warning signs:** Test case where client has `include_usage: Some(false)` -- ensure it is preserved.

**Note on the locked decision:** The context says "preserve their settings and only add include_usage: true if missing (don't override)." This means if client sends `include_usage: false`, we respect that. The `is_none()` check in the merge function handles this correctly. However, this is a somewhat unusual choice -- if the client explicitly opts out of usage, arbstr respects it even though arbstr needs usage data. This is the locked decision and must be followed. Phase 10 will handle the case where no usage data arrives (UPDATE with NULLs).

### Pitfall 3: Forgetting skip_serializing_if on New Field

**What goes wrong:** Non-streaming requests suddenly include `"stream_options": null` in the upstream JSON, which some providers may reject.
**Why it happens:** Adding the field without the serde attribute.
**How to avoid:** Always use `#[serde(skip_serializing_if = "Option::is_none")]` on `stream_options` field.
**Warning signs:** Integration tests for non-streaming requests failing with unexpected fields.

### Pitfall 4: Clone Cost Concern (Non-Issue)

**What goes wrong:** Developer avoids cloning `ChatCompletionRequest` for performance, complicating the code with unsafe or refcell patterns.
**Why it happens:** Premature optimization anxiety.
**How to avoid:** Just clone. The request body is tiny compared to the network latency. The clone happens once per request.
**Warning signs:** Complex lifetime annotations or interior mutability patterns where a simple clone would suffice.

### Pitfall 5: Breaking Existing Tests

**What goes wrong:** Adding `stream_options` field to `ChatCompletionRequest` breaks existing test JSON that constructs requests without the field.
**Why it happens:** If the field is not `Option<T>` or not annotated with `skip_serializing_if`, deserialization of test fixtures without the field fails.
**How to avoid:** Use `Option<StreamOptions>` (which defaults to `None` when absent in JSON). All existing tests construct requests via JSON deserialization or struct literals -- `Option` fields default to `None` in both cases.
**Warning signs:** Compile errors in existing test code after adding the field.

## Code Examples

### OpenAI stream_options Request Format

The canonical request format when stream_options is enabled:

```json
{
  "model": "gpt-4o",
  "messages": [{"role": "user", "content": "Hello"}],
  "stream": true,
  "stream_options": {
    "include_usage": true
  }
}
```

Source: [OpenAI Developer Community - Usage stats in streaming](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156)

### Final Usage Chunk from Provider

When `stream_options.include_usage` is true, the provider sends an extra final chunk before `data: [DONE]`:

```json
{
  "id": "chatcmpl-abc123",
  "object": "chat.completion.chunk",
  "created": 1693600060,
  "model": "gpt-4o",
  "choices": [],
  "usage": {
    "prompt_tokens": 6,
    "completion_tokens": 10,
    "total_tokens": 16
  }
}
```

Key observations:
- `choices` is an empty array (not absent)
- `usage` contains the standard three fields
- All intermediate chunks have `"usage": null` (or usage absent)
- This is the chunk Phase 9 will parse -- Phase 8 just ensures it gets sent

Source: [OpenAI Developer Community](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156)

### sqlx UPDATE with rows_affected

```rust
let result = sqlx::query("UPDATE requests SET input_tokens = ? WHERE correlation_id = ?")
    .bind(100i64)
    .bind("some-correlation-id")
    .execute(&pool)
    .await?;

let affected = result.rows_affected(); // u64
```

Source: [sqlx docs - Query struct](https://docs.rs/sqlx/latest/sqlx/query/struct.Query.html)

### wiremock Request Body Inspection (Integration Testing)

```rust
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};
use wiremock::matchers::body_json_string;

// Custom matcher that inspects JSON body
struct ContainsStreamOptions;

impl wiremock::Match for ContainsStreamOptions {
    fn matches(&self, request: &wiremock::Request) -> bool {
        if let Ok(body) = serde_json::from_slice::<serde_json::Value>(&request.body) {
            body.get("stream_options")
                .and_then(|so| so.get("include_usage"))
                .and_then(|v| v.as_bool())
                == Some(true)
        } else {
            false
        }
    }
}
```

Source: [wiremock-rs documentation](https://docs.rs/wiremock/)

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| No usage data in streaming | `stream_options.include_usage` sends usage in final chunk | OpenAI API, May 2024 | Streaming requests can now get accurate token counts without counting locally |
| `usage` absent from chunks | All chunks have `"usage": null`, final chunk has real data | Same update | Consistent field presence simplifies parsing |

**Deprecated/outdated:**
- Manual token counting from streaming content is no longer necessary when provider supports `stream_options`
- The existing code in `handle_streaming_response` (lines 671-707) already has a debug log for usage extraction from chunks but cannot act on it -- Phase 8 ensures the provider actually sends this data

## Open Questions

1. **Client opt-out of include_usage**
   - What we know: The locked decision says "preserve their settings and only add include_usage if missing." If client sends `include_usage: false`, we preserve it.
   - What's unclear: Does any real client ever send `include_usage: false`? This seems unlikely but the merge logic handles it.
   - Recommendation: Follow the locked decision exactly. Phase 10 handles the NULL-usage case anyway.

2. **Awaited INSERT latency impact on TTFB**
   - What we know: SQLite WAL mode INSERT is typically <1ms. Awaiting it adds negligible latency.
   - What's unclear: Under extreme write contention, could this block?
   - Recommendation: The SQLite pool has 5 connections. With WAL mode and NORMAL synchronous, write contention is unlikely. Proceed as designed.

## Sources

### Primary (HIGH confidence)
- **Codebase inspection:** `src/proxy/types.rs` (ChatCompletionRequest struct), `src/proxy/handlers.rs` (send_to_provider, streaming path), `src/storage/logging.rs` (spawn_log_write pattern, RequestLog struct), `migrations/20260203000000_initial_schema.sql` (schema confirms nullable columns)
- **OpenAI API docs:** [Usage stats in streaming](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156) -- confirmed `stream_options` format and final chunk structure

### Secondary (MEDIUM confidence)
- **sqlx docs:** [Query struct - execute and rows_affected](https://docs.rs/sqlx/latest/sqlx/query/struct.Query.html) -- confirmed UPDATE pattern
- **serde docs:** [Field attributes - skip_serializing_if](https://serde.rs/field-attrs.html) -- confirmed Option handling
- **wiremock-rs:** [Custom Match trait for request body inspection](https://docs.rs/wiremock/) -- confirmed integration test approach

### Tertiary (LOW confidence)
- None. All findings verified against codebase or official documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies, all patterns exist in codebase
- Architecture: HIGH -- injection point clearly identified in `send_to_provider`, UPDATE pattern mirrors existing INSERT
- Pitfalls: HIGH -- race condition and merge semantics are well-understood; mitigations are straightforward
- Testing: HIGH -- wiremock already in dev-deps, in-memory SQLite available via `init_pool(":memory:")`

**Research date:** 2026-02-15
**Valid until:** 2026-03-15 (stable -- OpenAI stream_options API unlikely to change, codebase patterns are settled)
