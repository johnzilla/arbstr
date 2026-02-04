# Phase 2: Request Logging - Research

**Researched:** 2026-02-03
**Domain:** SQLite storage with async request logging, token extraction, and latency tracking in a Rust/axum proxy
**Confidence:** HIGH

## Summary

Phase 2 adds persistent request logging to arbstr using SQLite via sqlx. Every completed request (success, failure, or pre-route rejection) is logged with timestamp, model, provider, token counts, costs, latency, and the correlation ID from Phase 1. The implementation uses sqlx's embedded migration system (`migrate!()` macro) to auto-apply schema on startup, `SqlitePool` added to `AppState` for shared access, and `tokio::spawn` fire-and-forget writes so database operations never block the response path.

The codebase already has `sqlx 0.8.6` with `sqlite`, `runtime-tokio`, `migrate`, and `macros` features enabled. No new dependencies are needed. The key technical challenges are: (1) extracting the correlation ID from the tracing span into handler-accessible form (requires adding request extensions alongside the existing span), (2) extracting token usage from non-streaming responses (parsing the `usage` object from provider JSON), (3) intercepting streaming SSE chunks to capture usage from the final chunk without buffering, and (4) structuring the fire-and-forget write to own all data before spawning.

**Primary recommendation:** Create a `src/storage/` module with migration SQL in `migrations/`, add `SqlitePool` to `AppState`, extend the `make_span_with` middleware to also store the UUID in request extensions, use `sqlx::query()` (runtime function, not compile-time macro) for INSERT statements with `?` bind parameters, and spawn writes via `tokio::spawn` with cloned pool and owned data.

## Standard Stack

### Core (already in Cargo.toml -- no additions needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `sqlx` | 0.8.6 | SQLite connection pool, migrations, queries | Already in deps with `sqlite`, `runtime-tokio`, `migrate` features |
| `tokio` | 1.x | `tokio::spawn` for fire-and-forget writes, `Instant` for latency | Already the async runtime |
| `uuid` | 1.x (v4) | Correlation IDs (from Phase 1) | Already in deps |
| `chrono` | 0.4.x (serde) | ISO 8601 timestamps for log entries | Already in deps |
| `tracing` | 0.1.x | Logging write failures as warnings | Already in deps |
| `serde_json` | 1.x | Parsing usage object from provider responses | Already in deps |

### Supporting (no changes needed)

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `axum` | 0.7.x | `State` extractor for pool access, `Extension` for correlation ID | Request handlers |
| `futures` | 0.3 | `StreamExt::map` for intercepting streaming chunks | Streaming token extraction |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `sqlx::query()` runtime function | `sqlx::query!()` compile-time macro | Macro provides compile-time SQL validation but requires `DATABASE_URL` at build time and a running/accessible database. For INSERT-only logging, the runtime function is simpler and the SQL is straightforward enough that compile-time checking adds build complexity without proportional safety benefit. |
| `chrono::Utc::now().to_rfc3339()` for timestamps | SQLite `datetime('now')` function | Using chrono gives arbstr control over the timestamp (measured at response completion, not at write time if write is delayed). Also avoids relying on SQLite server-side time. |
| Single pool for reads and writes | Separate reader/writer pools | Overkill for single-user tool with low write volume. A single pool with `max_connections` of 2-5 is sufficient. |

**No new dependencies needed.** All existing crate versions and features are sufficient.

## Architecture Patterns

### Recommended Project Structure

```
src/
├── storage/
│   ├── mod.rs           # Module root, re-exports, pool initialization
│   └── logging.rs       # RequestLog struct, insert function
migrations/
│   └── 20260203000000_initial_schema.sql  # Schema for requests + token_ratios tables
build.rs                 # cargo:rerun-if-changed=migrations (for migrate! macro)
```

### Pattern 1: SqlitePool in AppState

**What:** Add `SqlitePool` (wrapped in `Option`) to the shared `AppState` struct so all handlers can access it via axum's `State` extractor.
**When to use:** Every request handler that logs to the database.

```rust
// Source: sqlx docs, axum State extractor pattern
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: Option<SqlitePool>,  // None when DB is disabled
}
```

The pool is `Option<SqlitePool>` to handle the case where database config is absent. `SqlitePool` implements `Clone` cheaply (it's `Arc`-based internally).

### Pattern 2: Pool Initialization with WAL Mode

**What:** Create the `SqlitePool` at startup with WAL journal mode and `create_if_missing(true)`, then run embedded migrations.
**When to use:** During server startup in `run_server()`.

```rust
// Source: sqlx 0.8 docs - SqliteConnectOptions, SqlitePool
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::str::FromStr;

let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
    .journal_mode(SqliteJournalMode::Wal)
    .synchronous(SqliteSynchronous::Normal)  // Safe with WAL, much faster than FULL
    .create_if_missing(true);

let pool = SqlitePoolOptions::new()
    .max_connections(5)
    .connect_with(opts)
    .await?;

// Run embedded migrations
sqlx::migrate!().run(&pool).await?;
```

**Key detail:** `create_if_missing(true)` is required -- sqlx's `SqliteConnectOptions` defaults to NOT creating the file if it doesn't exist. Without this, first-time startup with no pre-existing database will fail.

**WAL mode rationale:** WAL (Write-Ahead Logging) allows concurrent reads and writes. With `synchronous=Normal`, writes are fast (tens of thousands of inserts per second) while still being safe against database corruption from crashes (though the last transaction before a crash may be lost -- acceptable for logging).

### Pattern 3: Correlation ID in Request Extensions

**What:** Store the UUID in both the tracing span (for log output) AND in request extensions (for programmatic access in handlers).
**When to use:** Modify the existing `make_span_with` closure in `create_router()`.

```rust
// Source: axum Extension docs, tower-http TraceLayer
use axum::extract::Extension;
use uuid::Uuid;

// Newtype for type safety in extensions
#[derive(Clone, Debug)]
pub struct RequestId(pub Uuid);

// In create_router(), add a middleware layer that inserts the RequestId
// before TraceLayer, or modify TraceLayer to also store in extensions.
//
// Option A: Separate middleware before TraceLayer
axum::middleware::from_fn(|mut request: axum::http::Request<axum::body::Body>, next: axum::middleware::Next| async move {
    let request_id = Uuid::new_v4();
    request.extensions_mut().insert(RequestId(request_id));
    // The tracing span from TraceLayer will use the same UUID
    next.run(request).await
})

// Option B: Generate UUID in make_span_with AND store in extensions
// This is trickier because make_span_with receives an immutable &Request.
// The middleware approach (Option A) is cleaner.
```

**Critical insight:** The tracing span system does NOT allow reading span field values back programmatically. `Span::current()` has no getter for field values like `request_id`. For Phase 2, we need the correlation ID as a `String` value to INSERT into SQLite. The solution is to generate the UUID in a middleware that stores it in request extensions, then reference the same UUID in the tracing span.

**Recommended approach:** Use `axum::middleware::from_fn` to generate the UUID and store it in extensions. Then modify `make_span_with` to read the UUID from extensions rather than generating a new one. This keeps both the span and the handler using the same UUID.

```rust
// In middleware (runs first, sets extension):
let request_id = Uuid::new_v4();
request.extensions_mut().insert(RequestId(request_id));

// In make_span_with (reads from extension):
let request_id = request.extensions()
    .get::<RequestId>()
    .map(|r| r.0)
    .unwrap_or_else(Uuid::new_v4);  // fallback
tracing::info_span!("request", request_id = %request_id, ...)
```

### Pattern 4: Fire-and-Forget Database Write

**What:** After the response is ready, spawn a background task to write the log entry. Clone the pool and move owned data into the spawned future.
**When to use:** At the end of the `chat_completions` handler, after the response is constructed.

```rust
// Source: tokio::spawn docs, sqlx pool clone pattern
if let Some(pool) = &state.db {
    let pool = pool.clone();
    let log_entry = RequestLog { /* owned fields */ };
    tokio::spawn(async move {
        if let Err(e) = log_entry.insert(&pool).await {
            tracing::warn!(error = %e, "Failed to log request to database");
        }
    });
}
```

**Key constraint for `tokio::spawn`:** Everything moved into the closure must be `'static`. This means:
- `pool.clone()` -- SqlitePool is Arc-based, clone is cheap
- All log entry fields must be owned (`String`, not `&str`)
- SQL string literals are `&'static str`, so `sqlx::query("INSERT ...")` is fine

### Pattern 5: Token Extraction from Non-Streaming Response

**What:** After parsing the provider's JSON response, extract the `usage` object to get token counts.
**When to use:** In the non-streaming path of `chat_completions`.

```rust
// Source: OpenAI API reference - chat completion response format
// The response JSON has: { "usage": { "prompt_tokens": N, "completion_tokens": N, "total_tokens": N } }

let usage = response.get("usage").and_then(|u| {
    Some((
        u.get("prompt_tokens")?.as_u64()? as u32,
        u.get("completion_tokens")?.as_u64()? as u32,
    ))
});

let (input_tokens, output_tokens) = match usage {
    Some((input, output)) => (Some(input), Some(output)),
    None => (None, None),  // Log with null tokens if missing
};
```

Per CONTEXT.md decision: if usage is missing or incomplete, log with null token/cost fields.

### Pattern 6: Streaming Token Extraction (Chunk Interception)

**What:** Intercept SSE chunks as they pass through to the client, parse the final chunk for usage data, without buffering the entire stream.
**When to use:** In the streaming path of `chat_completions`.

```rust
// Source: OpenAI streaming format, futures StreamExt
// SSE format: each message is "data: {json}\n\n" or "data: [DONE]\n\n"
// Final chunk (when stream_options.include_usage=true):
//   data: {"id":"...","choices":[],"usage":{"prompt_tokens":N,"completion_tokens":N,"total_tokens":N}}

use std::sync::Arc;
use tokio::sync::Mutex;

// Shared state for capturing usage from stream
let captured_usage: Arc<Mutex<Option<(u32, u32)>>> = Arc::new(Mutex::new(None));
let captured_usage_clone = captured_usage.clone();

let stream = upstream_response.bytes_stream().map(move |chunk| {
    if let Ok(ref bytes) = chunk {
        // Try to parse SSE data lines from this chunk
        if let Ok(text) = std::str::from_utf8(bytes) {
            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data != "[DONE]" {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(usage) = parsed.get("usage").filter(|u| !u.is_null()) {
                                if let (Some(input), Some(output)) = (
                                    usage.get("prompt_tokens").and_then(|v| v.as_u64()),
                                    usage.get("completion_tokens").and_then(|v| v.as_u64()),
                                ) {
                                    // Store usage -- this will be read after stream completes
                                    let usage_ref = captured_usage_clone.clone();
                                    // Note: we're in a sync map closure, use try_lock
                                    if let Ok(mut guard) = usage_ref.try_lock() {
                                        *guard = Some((input as u32, output as u32));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    chunk.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
});
```

**Important caveats for streaming chunk parsing:**
1. SSE chunks from reqwest may not align with SSE message boundaries -- a single `bytes_stream()` chunk may contain partial lines or multiple complete lines.
2. The `data: ` prefix parsing must handle line boundaries correctly.
3. Not all providers send usage in the final chunk -- some require `stream_options: {"include_usage": true}` in the request. Since arbstr forwards the client's request as-is, usage may or may not be present.
4. If usage is not captured from the stream, log with null tokens/cost (same as missing usage in non-streaming).

### Pattern 7: Latency Measurement

**What:** Use `std::time::Instant` to measure wall-clock time from request receipt to response completion.
**When to use:** Start timer at beginning of handler, record elapsed when response is ready.

```rust
// Source: std::time::Instant documentation
let start = std::time::Instant::now();
// ... handle request ...
let latency_ms = start.elapsed().as_millis() as i64;
```

For streaming, latency reflects time until the full stream has been consumed (measured when the stream completes, not when headers are sent).

### Anti-Patterns to Avoid

- **Using `sqlx::query!()` macro for INSERT statements:** Requires `DATABASE_URL` at compile time, which means a database file must exist during `cargo build`. This adds build-time complexity for no significant benefit on INSERT-only logging queries. Use the runtime `sqlx::query()` function instead.
- **Blocking on database writes in the response path:** Never `await` the database write before returning the response. Use `tokio::spawn` to decouple the write from the response.
- **Creating the database pool inside each handler:** The pool must be created once at startup and shared via `AppState`. `SqlitePool::clone()` is a cheap Arc clone.
- **Using `synchronous=FULL` with WAL mode:** `FULL` forces a sync after every transaction, destroying write performance. `NORMAL` is the correct setting for WAL mode -- it provides crash safety for the database file while allowing fast writes.
- **Forgetting `create_if_missing(true)` on SqliteConnectOptions:** Without this, first-time startup fails because the database file doesn't exist yet. SQLite's default through sqlx is to NOT auto-create.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Database migrations | Custom SQL execution at startup | `sqlx::migrate!()` macro with `.run(&pool)` | Tracks applied migrations, handles versioning, embeds SQL in binary |
| Connection pooling | Single connection with manual locking | `SqlitePool` with `SqlitePoolOptions` | Handles connection lifecycle, health checks, concurrent access |
| WAL mode configuration | Manual `PRAGMA` execution after connect | `SqliteConnectOptions::journal_mode(SqliteJournalMode::Wal)` | Applied before connection is usable, type-safe configuration |
| SSE line parsing | Full SSE parser with event types | Simple `strip_prefix("data: ")` on UTF-8 text | arbstr only needs to read `data:` lines, not implement full SSE spec |
| Timestamp formatting | Manual string formatting | `chrono::Utc::now().to_rfc3339()` | Already in deps, handles timezone and formatting correctly |

**Key insight:** For the fire-and-forget logging pattern, the complexity is in data ownership (ensuring all values are owned before `tokio::spawn`), not in the SQL or pool management. Focus implementation effort on cleanly structuring the `RequestLog` data type so all fields are owned.

## Common Pitfalls

### Pitfall 1: Correlation ID Not Accessible in Handler

**What goes wrong:** The UUID is generated inside `make_span_with` and stored only as a tracing span field. Handlers cannot read span field values programmatically (tracing deliberately does not expose span field values via `Span::current()`).
**Why it happens:** Phase 1 designed the correlation ID for tracing logs only, not for programmatic access.
**How to avoid:** Generate the UUID in a middleware layer that stores it in request extensions AND passes it to the span. Use `axum::middleware::from_fn` to insert `RequestId(uuid)` into extensions before TraceLayer runs.
**Warning signs:** Handler can see `Span::current()` but has no way to extract the `request_id` string value from it.

### Pitfall 2: Non-'static Data in tokio::spawn

**What goes wrong:** `tokio::spawn` requires `'static` futures. If the log entry contains borrowed data (like `&str` from the request), the compiler rejects it.
**Why it happens:** The spawned task may outlive the handler's stack frame.
**How to avoid:** Structure `RequestLog` with all owned types (`String`, not `&str`). Clone or `.to_string()` all borrowed values before constructing the log entry. Clone the pool handle.
**Warning signs:** Compiler errors about lifetimes not satisfying `'static` bound on `tokio::spawn`.

### Pitfall 3: SSE Chunk Boundary Misalignment

**What goes wrong:** A single `bytes_stream()` chunk from reqwest may contain partial SSE lines or span multiple SSE messages. Naive line-by-line parsing of individual chunks misses data that spans chunk boundaries.
**Why it happens:** TCP segments and HTTP chunked transfer don't align with SSE message boundaries. A single SSE line like `data: {"usage":...}` could be split across two chunks.
**How to avoid:** Buffer incomplete lines across chunks (append to a running buffer, split on `\n`, process complete lines, keep the trailing incomplete portion). Alternatively, for the usage-only use case, search for the `"usage"` key in the raw text of each chunk -- if the JSON is split across chunks, the usage will appear in the chunk containing the closing brace.
**Warning signs:** Usage occasionally not captured from streams, especially under network conditions that produce small TCP segments.

### Pitfall 4: Missing create_if_missing on SqliteConnectOptions

**What goes wrong:** First-time startup fails with "unable to open database file" error.
**Why it happens:** sqlx's `SqliteConnectOptions` defaults `create_if_missing` to `false`. The CONTEXT.md says "DB file auto-created by SQLite on first connection" but this requires explicit opt-in through sqlx.
**How to avoid:** Always call `.create_if_missing(true)` when building `SqliteConnectOptions`.
**Warning signs:** "unable to open database file" error on first run with no pre-existing `.db` file.

### Pitfall 5: build.rs Missing for Migration Changes

**What goes wrong:** After adding or modifying migration SQL files, `cargo build` doesn't recompile because the compiler doesn't watch the `migrations/` directory by default.
**Why it happens:** The `migrate!()` proc macro reads files at compile time, but Rust's incremental compilation doesn't know to re-run the macro when non-source files change.
**How to avoid:** Create a `build.rs` with `println!("cargo:rerun-if-changed=migrations");` or run `sqlx migrate build-script` to generate it.
**Warning signs:** Migration changes not taking effect after `cargo build` (requires `cargo clean` to force).

### Pitfall 6: Logging Failed Requests That Return Early via Error

**What goes wrong:** Failed requests (provider errors, routing rejections) return `Err(Error)` from the handler before reaching the logging code, so they're never logged.
**Why it happens:** The happy path constructs the log entry at the end. Error paths use `?` or `return Err(...)` and skip logging.
**How to avoid:** Structure the handler so logging happens in all code paths. One approach: use a helper function or closure that always logs, wrapping the core logic in a result. Another: log in the error conversion path (though this couples error handling to database state).
**Warning signs:** Database contains only successful requests; failed requests have no entries.

### Pitfall 7: Pool Exhaustion from Slow Writes

**What goes wrong:** If many concurrent requests spawn fire-and-forget writes and the database is slow (e.g., on a slow filesystem), the pool connections fill up and new writes block waiting for a connection, eventually timing out.
**Why it happens:** `tokio::spawn` fire-and-forget means writes accumulate without backpressure.
**How to avoid:** Use a reasonable pool size (5 connections) and a short `acquire_timeout` (5 seconds). If a connection isn't available, the write fails and the warning is logged. For a single-user tool, this is unlikely to be an issue in practice.
**Warning signs:** "pool timed out" errors in logs under load.

## Code Examples

### Example 1: Migration SQL (Initial Schema)

```sql
-- Source: CONTEXT.md schema decisions
-- migrations/20260203000000_initial_schema.sql

-- Request log for cost tracking and observability
CREATE TABLE IF NOT EXISTS requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    correlation_id TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    model TEXT NOT NULL,
    provider TEXT,
    policy TEXT,
    streaming BOOLEAN NOT NULL DEFAULT FALSE,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_sats REAL,
    provider_cost_sats REAL,
    latency_ms INTEGER NOT NULL,
    success BOOLEAN NOT NULL,
    error_status INTEGER,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_requests_correlation_id ON requests(correlation_id);
CREATE INDEX IF NOT EXISTS idx_requests_timestamp ON requests(timestamp);

-- Learned input/output ratios per policy (populated in future phases)
CREATE TABLE IF NOT EXISTS token_ratios (
    policy TEXT PRIMARY KEY,
    avg_ratio REAL NOT NULL,
    sample_count INTEGER NOT NULL DEFAULT 0
);
```

**Column type decisions:**
- `id`: INTEGER PRIMARY KEY AUTOINCREMENT -- auto-increment per CONTEXT.md
- `correlation_id`: TEXT NOT NULL, indexed -- UUID as string per CONTEXT.md
- `timestamp`: TEXT -- ISO 8601 string (SQLite has no native datetime type; TEXT is the standard approach)
- `model`: TEXT NOT NULL -- always known from the request
- `provider`: TEXT nullable -- null for pre-route rejections (no provider selected)
- `cost_sats`: REAL -- f64 for sub-satoshi precision per Phase 1 decision
- `provider_cost_sats`: REAL nullable -- provider-reported cost, when available
- `streaming`: BOOLEAN NOT NULL -- per CONTEXT.md
- `error_status`: INTEGER nullable -- HTTP status code for errors
- `error_message`: TEXT nullable -- short description for errors

### Example 2: RequestLog Struct and Insert

```rust
// Source: Derived from CONTEXT.md schema decisions

/// A completed request log entry ready for database insertion.
pub struct RequestLog {
    pub correlation_id: String,
    pub timestamp: String,
    pub model: String,
    pub provider: Option<String>,
    pub policy: Option<String>,
    pub streaming: bool,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cost_sats: Option<f64>,
    pub provider_cost_sats: Option<f64>,
    pub latency_ms: i64,
    pub success: bool,
    pub error_status: Option<u16>,
    pub error_message: Option<String>,
}

impl RequestLog {
    pub async fn insert(&self, pool: &SqlitePool) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO requests (
                correlation_id, timestamp, model, provider, policy,
                streaming, input_tokens, output_tokens,
                cost_sats, provider_cost_sats,
                latency_ms, success, error_status, error_message
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&self.correlation_id)
        .bind(&self.timestamp)
        .bind(&self.model)
        .bind(&self.provider)
        .bind(&self.policy)
        .bind(self.streaming)
        .bind(self.input_tokens.map(|v| v as i64))
        .bind(self.output_tokens.map(|v| v as i64))
        .bind(self.cost_sats)
        .bind(self.provider_cost_sats)
        .bind(self.latency_ms)
        .bind(self.success)
        .bind(self.error_status.map(|v| v as i32))
        .bind(self.error_message.as_deref())
        .execute(pool)
        .await?;
        Ok(())
    }
}
```

### Example 3: Pool Initialization and Migration

```rust
// Source: sqlx 0.8 docs
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use std::str::FromStr;

pub async fn init_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    // Apply embedded migrations
    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}
```

### Example 4: Fire-and-Forget Write Pattern

```rust
// Source: tokio::spawn docs, sqlx pool pattern
fn spawn_log_write(pool: &SqlitePool, log: RequestLog) {
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Err(e) = log.insert(&pool).await {
            tracing::warn!(
                correlation_id = %log.correlation_id,
                error = %e,
                "Failed to write request log to database"
            );
        }
    });
}
```

### Example 5: Usage Extraction from Non-Streaming Response

```rust
// Source: OpenAI API response format
fn extract_usage(response: &serde_json::Value) -> Option<(u32, u32)> {
    let usage = response.get("usage")?;
    let input = usage.get("prompt_tokens")?.as_u64()? as u32;
    let output = usage.get("completion_tokens")?.as_u64()? as u32;
    Some((input, output))
}
```

### Example 6: build.rs for Migration Recompilation

```rust
// build.rs (project root)
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
```

## State of the Art

| Old Approach (current code) | Current Approach (Phase 2 target) | Impact |
|-----|------|--------|
| `TODO: Log to database for cost tracking` comment in handler | Every request logged to SQLite with full metadata | Complete observability for cost analysis |
| No database initialization | `SqlitePool` in `AppState`, migrations on startup | Zero-config database setup |
| No token extraction | Parse `usage` object from provider JSON | Accurate cost calculation per request |
| UUID only in tracing span | UUID in both span AND request extensions | Correlation ID accessible for DB logging AND log output |
| No latency tracking | `Instant::now()` at handler start, elapsed at end | Wall-clock latency per request |
| No failure logging | All paths (success, error, rejection) log to same table | Complete request audit trail |

**Deprecated/outdated:**
- The `TODO: Log to database for cost tracking` comment in `handlers.rs` (line 121) will be replaced with actual logging code.
- The `ChatCompletionChunk` type in `types.rs` lacks a `usage` field -- it needs to be added (or streaming usage extracted via raw JSON parsing) for streaming token extraction.

## Open Questions

1. **Streaming chunk boundary handling**
   - What we know: reqwest `bytes_stream()` chunks don't align with SSE message boundaries. The usage JSON could span two chunks.
   - What's unclear: How often chunk splitting occurs in practice for the final usage chunk (it's typically small and comes as a single TCP segment).
   - Recommendation: Implement a line buffer that accumulates partial lines across chunks. This is more robust than assuming chunk-message alignment, but adds modest complexity. Alternatively, start with the simple approach (parse complete lines within each chunk) and accept that usage might occasionally not be captured from streams -- the fallback is null tokens in the log, which is the same behavior as a provider that doesn't send usage.

2. **Streaming latency semantics**
   - What we know: Non-streaming latency is clear (request receipt to response body ready). For streaming, latency could mean time-to-first-byte or time-to-stream-completion.
   - What's unclear: Which is more useful for a cost optimization proxy.
   - Recommendation: Log time-to-first-byte as `latency_ms` for streaming requests. This reflects the user-perceived responsiveness. Time-to-completion is less meaningful because it depends on output length, not provider performance. However, the fire-and-forget write happens after stream completion anyway, so measuring stream completion latency is actually easier to implement. Either approach is valid for Phase 2.

3. **Error type additions for database operations**
   - What we know: The current `Error` enum in `error.rs` has no database-related variant. Database errors during pool init would need to be propagated as `anyhow::Error` in `run_server()` or mapped to `Error::Internal`.
   - What's unclear: Whether to add a `Database(sqlx::Error)` variant to the main `Error` enum or keep DB errors separate (since they only occur in the write path, which is fire-and-forget).
   - Recommendation: Add a `Database(sqlx::Error)` variant. Pool initialization errors need to propagate at startup (fatal). Write errors in fire-and-forget can be logged as warnings without converting to the application error type, but having the variant available is good for future use (query endpoints in later phases).

## Sources

### Primary (HIGH confidence)
- [sqlx 0.8 `migrate!()` macro docs](https://docs.rs/sqlx/0.8/sqlx/macro.migrate.html) - Embedded migration system, `./migrations` default directory, `build.rs` requirement
- [sqlx 0.8 `Migrator` docs](https://docs.rs/sqlx/0.8/sqlx/migrate/struct.Migrator.html) - `run()` method, migration execution
- [sqlx 0.8 `SqliteConnectOptions` docs](https://docs.rs/sqlx/0.8/sqlx/sqlite/struct.SqliteConnectOptions.html) - WAL mode, `create_if_missing`, synchronous mode, journal mode configuration
- [sqlx 0.8 `PoolOptions` docs](https://docs.rs/sqlx/0.8/sqlx/pool/struct.PoolOptions.html) - `max_connections`, `acquire_timeout`, pool configuration
- [sqlx 0.8 `query()` function docs](https://docs.rs/sqlx/0.8/sqlx/fn.query.html) - Runtime query execution, `?` bind parameters for SQLite
- Local codebase analysis - all source files read and verified, sqlx 0.8.6 with `sqlite,runtime-tokio,migrate` features confirmed via `cargo tree`
- `cargo test` output - 8 tests pass, confirming current baseline

### Secondary (MEDIUM confidence)
- [OpenAI streaming usage stats announcement](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156) - `stream_options.include_usage`, final chunk format with empty `choices` and populated `usage`
- [axum discussion #2273](https://github.com/tokio-rs/axum/discussions/2273) - Request ID in tracing spans, maintainer confirmation of span isolation
- [SQLite WAL mode best practices (drmhse.com)](https://www.drmhse.com/posts/battling-with-sqlite-in-a-concurrent-environment/) - Separate reader/writer pools pattern, WAL+NORMAL synchronous recommendation
- [sqlx migration file naming (studyraid.com)](https://app.studyraid.com/en/read/14938/515209/creating-and-applying-migrations) - `{version}_{description}.sql` naming convention
- [axum Extension docs](https://docs.rs/axum/latest/axum/struct.Extension.html) - Extension as extractor and layer, `extensions_mut().insert()` pattern
- [Rust tracing span field access discussion](https://users.rust-lang.org/t/access-field-value-of-a-span/56711) - Confirmed: tracing does NOT expose span field values via `Span::current()`

### Tertiary (LOW confidence)
- Streaming chunk boundary behavior -- Based on general TCP/HTTP knowledge; not empirically verified for specific providers in the Routstr network. The assumption is that small JSON usage chunks typically arrive in a single TCP segment, but this is not guaranteed.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - All dependencies already in Cargo.toml with correct features, APIs verified against docs.rs
- Architecture: HIGH - Patterns derived from sqlx official docs, axum State/Extension patterns used by codebase, fire-and-forget via `tokio::spawn` is well-documented
- Pitfalls: HIGH - Correlation ID accessibility issue verified by checking tracing API docs (no span field getters exist), `create_if_missing` default verified in sqlx docs, `build.rs` requirement documented in `migrate!()` macro docs

**Research date:** 2026-02-03
**Valid until:** 2026-03-03 (stable domain -- Rust ecosystem, no fast-moving dependencies)
