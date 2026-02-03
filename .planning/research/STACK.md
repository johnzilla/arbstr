# Technology Stack: Reliability & Observability for arbstr

**Project:** arbstr - LLM routing proxy reliability and observability milestone
**Researched:** 2026-02-02
**Overall confidence:** MEDIUM (training data only; web search/Context7 unavailable for version verification)

## Existing Stack (No Changes Needed)

These dependencies are already in `Cargo.toml` and remain correct for this milestone:

| Technology | Version | Purpose | Status |
|------------|---------|---------|--------|
| tokio | 1.x (full) | Async runtime | Keep as-is |
| axum | 0.7 | HTTP server | Keep as-is |
| reqwest | 0.12 (json, stream) | HTTP client to providers | Keep as-is |
| serde / serde_json | 1.x | Serialization | Keep as-is |
| tracing / tracing-subscriber | 0.1 / 0.3 | Structured logging | Keep as-is |
| thiserror | 1.x | Error types | Keep as-is |
| anyhow | 1.x | CLI error handling | Keep as-is |
| chrono | 0.4 | Timestamps | Keep as-is (will be used for request logging) |
| uuid | 1.x (v4) | Request IDs | Keep as-is (will be used for request logging) |
| futures | 0.3 | Stream combinators | Keep as-is (critical for streaming error handling) |
| tower | 0.4 | Middleware | Keep as-is |
| tower-http | 0.5 | HTTP middleware | Keep as-is |
| clap | 4.x | CLI | Keep as-is |
| toml / config | 0.8 / 0.14 | Configuration | Keep as-is |

## New Dependencies Required

### 1. SQLite Database Layer: sqlx (already in Cargo.toml)

| Technology | Version | Purpose | Confidence |
|------------|---------|---------|------------|
| sqlx | 0.8 | Async SQLite with compile-time checked queries | HIGH |

**Already declared** in `Cargo.toml` as `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite"] }` but never used. This is the right choice.

**Why sqlx:**
- Already chosen and declared in the project -- no reason to change
- Async-native, integrates cleanly with Tokio runtime already in use
- Compile-time query checking via `sqlx::query!` macro catches SQL errors at build time
- Built-in connection pooling via `SqlitePool`
- Built-in migration support via `sqlx::migrate!` macro
- WAL mode support for concurrent read/write (critical for logging while serving)

**Feature flags to add:** The current feature set `["runtime-tokio", "sqlite"]` is the minimum. Consider adding:
- `"migrate"` -- for embedded migration support via `sqlx::migrate!()` macro (runs migrations at startup from `migrations/` directory)
- `"chrono"` -- for native `chrono::DateTime` type mapping in queries (since chrono is already a dependency)

**Recommended Cargo.toml change:**
```toml
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate", "chrono"] }
```

**Confidence:** HIGH -- sqlx 0.8 is the standard async database crate for Rust. The project already chose it. The feature flags are well-documented patterns.

**Key patterns for this milestone:**
- Use `SqlitePool::connect()` with WAL mode enabled via `?mode=rwc` and PRAGMA settings
- Use `sqlx::migrate!()` macro with `migrations/` directory for schema versioning
- Use `SqlitePool` in `AppState` (shared via Arc, internally pooled)
- Use `sqlx::query!` or `sqlx::query_as!` for compile-time checked queries
- For high-throughput logging: batch inserts or spawn fire-and-forget tasks to avoid blocking the request path

**WAL mode configuration** (set on connection):
```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;
```

**Migration file structure:**
```
migrations/
  20260202000000_create_requests.sql
  20260202000001_create_token_ratios.sql
```

### 2. No New Retry/Fallback Crate -- Use Custom Logic

| Decision | Recommendation | Confidence |
|----------|---------------|------------|
| Retry mechanism | Custom retry loop in handler | HIGH |

**Why NOT tower-retry:**
- `tower::retry::Retry` is designed for retrying the _same_ request to the _same_ service. arbstr needs to retry with a _different provider_ on failure (fallback, not retry).
- Tower retry operates at the Service trait level. arbstr's retry logic needs access to the `Router` to select the next-cheapest provider, which doesn't fit the tower middleware model.
- Tower retry doesn't understand arbstr's provider selection semantics (skip the failed provider, try next cheapest).

**Why NOT backon:**
- `backon` is a good general-purpose retry crate with exponential backoff, but it also retries the _same_ operation. arbstr needs provider-aware fallback that changes the target on each attempt.
- Adding a dependency for something that doesn't match the use case adds complexity without value.

**Why custom:**
- arbstr's fallback is simple: try provider A, if it fails, ask router for provider B (excluding A), try B. This is 20-30 lines of Rust, not worth a dependency.
- The router already has `select()` logic. Fallback = call `select()` again with an exclusion list.
- Custom logic gives full control over: which errors trigger fallback (network error vs 429 vs 500), how many retries, whether to try same model or allow model change, streaming vs non-streaming behavior.

**Recommended pattern:**
```rust
// Pseudocode for fallback logic
let max_attempts = 3; // configurable
let mut excluded_providers: Vec<String> = vec![];

for attempt in 0..max_attempts {
    let provider = router.select(model, policy, prompt, &excluded_providers)?;
    match try_provider(&provider, &request).await {
        Ok(response) => return Ok(response),
        Err(e) if e.is_retryable() => {
            tracing::warn!(provider = %provider.name, attempt, error = %e, "Provider failed, trying fallback");
            excluded_providers.push(provider.name.clone());
            continue;
        }
        Err(e) => return Err(e), // Non-retryable (e.g., 400 Bad Request)
    }
}
```

**Retryable errors** (should trigger fallback):
- Connection refused / timeout (reqwest network errors)
- HTTP 429 (rate limited)
- HTTP 500, 502, 503, 504 (server errors)

**Non-retryable errors** (should NOT trigger fallback):
- HTTP 400 (bad request -- same request will fail everywhere)
- HTTP 401, 403 (auth failure -- provider-specific but not transient)
- HTTP 404 (model not found at provider)

**Confidence:** HIGH -- this is a well-understood pattern. Custom fallback with provider exclusion is simpler and more correct than adapting a generic retry crate.

### 3. No Circuit Breaker Crate -- Use Simple Provider Health State

| Decision | Recommendation | Confidence |
|----------|---------------|------------|
| Circuit breaker | Custom provider health tracking in AppState | MEDIUM |

**Why NOT a circuit breaker crate:**
- Full circuit breaker patterns (open/half-open/closed with configurable thresholds) are overkill for a single-user local proxy with 2-5 providers.
- Crates like `recloser` or tower-based circuit breakers add abstraction that doesn't match arbstr's provider model.

**Why simple health tracking:**
- Track per-provider failure counts and last-failure timestamps in a shared `DashMap` or `Arc<Mutex<HashMap>>`.
- During provider selection, deprioritize (or skip) providers that have failed recently.
- Reset failure count after a configurable cooldown period.
- This gives 80% of circuit breaker value with 20% of complexity.

**Recommended pattern:**
```rust
struct ProviderHealth {
    consecutive_failures: u32,
    last_failure: Option<Instant>,
}

// In router selection: skip providers with consecutive_failures > threshold
// AND last_failure within cooldown window (e.g., 60 seconds)
```

**Confidence:** MEDIUM -- the pattern is sound but the exact thresholds and recovery behavior will need tuning in practice. This is flagged as a potential area for phase-specific refinement.

### 4. Streaming Error Handling -- Use Existing futures + tokio

| Decision | Recommendation | Confidence |
|----------|---------------|------------|
| Stream error handling | futures::StreamExt with custom wrapper | HIGH |

**No new dependencies needed.** The `futures` crate (already at 0.3) provides all necessary stream combinators.

**Current problem** (from `src/proxy/handlers.rs:87-104`):
The streaming path does `upstream_response.bytes_stream().map(...)` which wraps reqwest errors as io::Error and silently passes them through. If the provider disconnects mid-stream, the client gets an incomplete response with no error indication.

**Recommended approach:**

1. **Parse SSE chunks as they arrive** -- instead of blindly forwarding bytes, parse each `data: {...}` line to extract token counts and detect errors.

2. **Detect mid-stream failures** -- if the upstream stream ends without a `[DONE]` sentinel or with an error chunk, inject an error SSE event before closing the client stream:
   ```
   data: {"error": {"message": "Provider disconnected mid-stream", "type": "arbstr_error"}}

   data: [DONE]
   ```

3. **Use `futures::stream::try_unfold` or manual state machine** to track streaming state:
   - Accumulate token count from streamed chunks
   - Detect `finish_reason: "stop"` to know completion was successful
   - On upstream error, emit error event to client
   - After stream ends (success or failure), log the request to SQLite

**Key pattern -- intercept stream for logging:**
```rust
// Wrap the byte stream to intercept chunks for token counting and logging
let (tx, rx) = tokio::sync::mpsc::channel(32);
tokio::spawn(async move {
    let mut total_chunks = 0;
    let mut finished_cleanly = false;

    while let Some(chunk) = upstream_stream.next().await {
        match chunk {
            Ok(bytes) => {
                // Parse SSE to detect [DONE] and count tokens
                if bytes_contain_done(&bytes) {
                    finished_cleanly = true;
                }
                total_chunks += 1;
                let _ = tx.send(Ok(bytes)).await;
            }
            Err(e) => {
                // Send error event to client
                let error_event = format_sse_error(&e);
                let _ = tx.send(Ok(error_event.into())).await;
                break;
            }
        }
    }

    // Log request after stream completes
    log_request(db, metadata, finished_cleanly).await;
});
```

**Confidence:** HIGH -- this pattern is well-established in Rust async proxies. No new dependencies needed; `futures::StreamExt` and `tokio::sync::mpsc` provide everything.

### 5. Observability: No New Metrics Crate -- Extend tracing + SQLite

| Decision | Recommendation | Confidence |
|----------|---------------|------------|
| Metrics approach | SQLite request log + tracing spans | HIGH |

**Why NOT the `metrics` crate or `prometheus`:**
- arbstr is a single-user local proxy. There is no Prometheus endpoint to scrape, no Grafana dashboard to feed, no ops team monitoring alerts.
- The `metrics` crate adds a runtime metrics registry, exporters, and recorder abstractions that are all wasted on a single-user system.
- SQLite-based logging gives durable, queryable history -- which is what a single user actually wants ("how much did I spend this week?").

**Why tracing + SQLite is sufficient:**
- `tracing` (already configured) handles real-time observability: debug-level span traces in the terminal show request flow, provider selection, and errors as they happen.
- SQLite (via sqlx) provides persistent observability: cost tracking, per-model breakdown, provider reliability stats, token ratio learning.
- Together they cover both "what's happening now" (tracing) and "what happened over time" (SQLite queries via CLI endpoints).

**Recommended approach:**

**tracing enhancements** (no new deps):
- Add structured spans per request with `request_id`, `model`, `provider`, `latency_ms`, `cost_sats` fields
- Use `tracing::info_span!` to create request-scoped spans that propagate through provider selection and forwarding
- Add `tracing-subscriber`'s JSON formatter for machine-parseable logs if needed later (already available in 0.3)

**SQLite logging** (using sqlx already in deps):
- Log every request: provider, model, input/output tokens, cost in sats, latency, success/failure
- Query endpoints: `/stats` for total spend, `/stats/models` for per-model breakdown
- Background write: spawn fire-and-forget task for DB insert to avoid blocking response

**Confidence:** HIGH -- this is the right approach for a single-user local tool. The metrics crate would be premature optimization toward infrastructure the user doesn't have.

### 6. Token Counting -- No tiktoken Dependency

| Decision | Recommendation | Confidence |
|----------|---------------|------------|
| Token counting | Extract from provider response, not local tokenization | HIGH |

**Why NOT tiktoken-rs or similar:**
- arbstr is a proxy. The upstream provider already counts tokens and reports them in the response (`usage.prompt_tokens`, `usage.completion_tokens`).
- Adding a local tokenizer (tiktoken-rs, ~10MB model files, different tokenizer per model) is complexity for zero accuracy gain -- the provider's count is authoritative.
- Local tokenization would also need to know which tokenizer each model uses (cl100k_base for GPT-4, etc.), which couples arbstr to specific model families.

**What to do instead:**
- **Non-streaming:** Parse `usage` field from the response JSON (already a `Usage` struct in `types.rs`). Use `prompt_tokens` and `completion_tokens` for cost calculation.
- **Streaming:** The final chunk in OpenAI-compatible SSE streams contains a `usage` field (since OpenAI's 2024 API update). Parse the last chunk before `[DONE]` to extract token counts. If the provider doesn't include usage in streaming, estimate from chunk count or prompt character length.
- **Fallback estimation:** For cost prediction _before_ seeing the response (used by router for selection), use character count heuristic: ~4 characters per token for English text. Store learned ratios per policy in `token_ratios` table (already in schema design).

**Confidence:** HIGH -- extracting token counts from provider responses is the standard pattern for proxy architectures.

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Database | sqlx 0.8 (SQLite) | rusqlite (sync) | sqlx already chosen; rusqlite is sync and would require spawn_blocking, adding complexity in async handlers |
| Database | sqlx 0.8 (SQLite) | sled (embedded KV) | Relational queries needed (SUM cost, GROUP BY model); sled is key-value only |
| Retry | Custom fallback loop | tower-retry 0.4 | Tower retry retries same service; arbstr needs to switch providers on failure |
| Retry | Custom fallback loop | backon | General retry; doesn't understand provider exclusion semantics |
| Circuit breaker | Custom health tracking | recloser | Full circuit breaker is overkill for 2-5 providers on a local proxy |
| Metrics | tracing + SQLite | metrics + prometheus | No Prometheus infrastructure; SQLite queries serve the single-user use case better |
| Token counting | Extract from response | tiktoken-rs | Provider response is authoritative; local tokenizer adds weight and model coupling |
| Streaming | futures::StreamExt | async-stream | async-stream adds macro dependency; futures StreamExt already available and sufficient |

## Summary of Cargo.toml Changes

**Minimal changes required.** Only one line needs updating:

```toml
# CHANGE: Add migrate and chrono features to sqlx
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate", "chrono"] }
```

**No new dependencies.** Everything needed for reliability (custom fallback, health tracking, streaming error handling) and observability (SQLite logging, tracing spans) is already in the dependency tree.

This is a deliberate and positive signal: the existing stack was well-chosen. The milestone is about _using_ what's already available, not adding new tools.

## Installation

```bash
# No new dependencies to install. Just update the sqlx feature flags:
# Edit Cargo.toml to add "migrate" and "chrono" to sqlx features, then:
cargo build

# For sqlx compile-time query checking (optional, improves DX):
# Install sqlx-cli for migration management
cargo install sqlx-cli --no-default-features --features sqlite

# Create initial migration
sqlx migrate add create_requests
sqlx migrate add create_token_ratios
```

## Architecture Integration Notes

**Where new code goes:**

| Component | Location | Purpose |
|-----------|----------|---------|
| Storage module | `src/storage/mod.rs` | SQLite pool setup, migration runner |
| Request logger | `src/storage/logger.rs` | Async request logging (fire-and-forget) |
| Provider health | `src/router/health.rs` | Per-provider failure tracking |
| Fallback logic | `src/proxy/handlers.rs` | Retry loop in chat_completions handler |
| Stream interceptor | `src/proxy/stream.rs` | Parse SSE chunks, extract tokens, detect errors |
| Stats endpoint | `src/proxy/handlers.rs` | `/stats` query endpoint |

**AppState additions:**
```rust
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: SqlitePool,               // NEW: database connection pool
    pub provider_health: Arc<ProviderHealthTracker>,  // NEW: failure tracking
}
```

## Sources and Confidence Notes

All recommendations are based on training data (cutoff: May 2025). Web search and Context7 were unavailable during this research session.

| Recommendation | Confidence | Verification Status |
|---------------|------------|---------------------|
| sqlx 0.8 with SQLite, migrate, chrono features | HIGH | Already in Cargo.toml; feature flags are well-documented in sqlx docs |
| Custom fallback over tower-retry | HIGH | Tower retry semantics are well-understood; provider fallback is a different pattern |
| Custom health tracking over circuit breaker crate | MEDIUM | Pattern is sound; thresholds need empirical tuning |
| futures::StreamExt for stream error handling | HIGH | Already a dependency; stream combinator patterns are stable |
| tracing + SQLite over metrics crate | HIGH | Single-user use case strongly favors queryable persistence over runtime counters |
| Token extraction from response over tiktoken | HIGH | Standard proxy pattern; OpenAI streaming usage field confirmed in training data |
| sqlx version 0.8 is current | MEDIUM | Was current as of training cutoff; verify no 0.9 release has occurred |
| No breaking changes in existing deps | LOW | Cannot verify without web access; cargo build will surface any issues |

**Gaps to verify when web access is available:**
- Confirm sqlx 0.8 is still the latest (check crates.io)
- Confirm OpenAI streaming `usage` field is supported by Routstr providers specifically
- Check if any new Rust retry/fallback crates have emerged since May 2025

---

*Stack research: 2026-02-02*
