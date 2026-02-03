# Architecture Patterns

**Domain:** Reliability and observability for Rust/Tokio/axum LLM proxy
**Researched:** 2026-02-02
**Overall confidence:** MEDIUM (based on codebase analysis and Rust ecosystem knowledge; WebSearch unavailable for external verification)

## Current Architecture (Baseline)

The existing arbstr proxy has a clean, simple flow:

```
Request → Handler → Router::select() → Forward to provider → Return response
```

Key traits of the current system:
- **Stateless**: No mutable shared state across requests (AppState is all Arc-wrapped immutable data)
- **Single-attempt**: One provider selected, one request sent, success or error
- **Fire-and-forget**: No post-request processing (the `// TODO: Log to database` comment in handlers.rs:120)
- **Pass-through streaming**: bytes_stream piped directly from provider to client with no inspection

This architecture needs to evolve to support retry/fallback, stream error detection, and async logging without losing its simplicity.

## Recommended Architecture

### Component Diagram

```
                              AppState
                    ┌──────────────────────────┐
                    │  router: Arc<Router>      │
                    │  http_client: Client      │
                    │  config: Arc<Config>      │
                    │  db: Arc<Storage>    [NEW] │
                    │  health: Arc<HealthTracker> [NEW] │
                    └──────────────────────────┘
                                │
Request ──► chat_completions handler
                │
                ├─► Router::select() → SelectedProvider
                │       (unchanged, still picks cheapest)
                │
                ├─► RequestExecutor::execute()         [NEW]
                │       │
                │       ├─► Attempt 1: forward to provider
                │       │       ├─► Success → response
                │       │       └─► Failure → retry decision
                │       │
                │       ├─► Attempt 2: forward (same or different provider)
                │       │       ├─► Success → response
                │       │       └─► Failure → error to client
                │       │
                │       └─► Return: RequestOutcome (response + metadata)
                │
                ├─► StreamWrapper::wrap()               [NEW]
                │       (for streaming: inspect chunks, detect errors)
                │
                └─► Storage::log_request()              [NEW]
                        (async, non-blocking, after response started)
```

### Component Boundaries

| Component | Responsibility | Location | Communicates With |
|-----------|---------------|----------|-------------------|
| **Handler** (existing) | HTTP endpoint, request parsing, response building | `src/proxy/handlers.rs` | Router, RequestExecutor, Storage |
| **Router** (existing) | Provider selection based on model/cost/policy | `src/router/selector.rs` | Config (read-only) |
| **RequestExecutor** (new) | Forward request to provider with retry/fallback logic | `src/proxy/executor.rs` | Router (for fallback selection), HTTP client, HealthTracker |
| **StreamWrapper** (new) | Wrap streaming responses to detect mid-stream errors | `src/proxy/stream.rs` | RequestExecutor output |
| **Storage** (new) | SQLite request logging and cost queries | `src/storage/mod.rs` | SQLite via sqlx |
| **HealthTracker** (new) | Track provider success/failure rates | `src/router/health.rs` | Updated by RequestExecutor, read by Router |
| **CostCalculator** (new) | Compute actual request cost from token counts | `src/router/cost.rs` | Used by Storage for logging |

### Data Flow

#### Non-Streaming Request (Happy Path)

```
1. Client POST /v1/chat/completions
2. Handler extracts model, policy, prompt
3. Router::select() → SelectedProvider (cheapest matching)
4. RequestExecutor::execute(provider, request)
   a. Build upstream request with auth headers
   b. Send to provider.url/chat/completions
   c. Receive full response
   d. Parse JSON, extract usage.prompt_tokens and usage.completion_tokens
   e. Return RequestOutcome { response, provider, tokens, latency, success: true }
5. Handler builds response with arbstr_provider header
6. Handler spawns tokio::spawn(storage.log_request(outcome))
7. Return response to client
```

#### Non-Streaming Request (Failure + Retry)

```
1-3. Same as happy path
4. RequestExecutor::execute(provider, request)
   a. Send to provider → timeout / 5xx / connection refused
   b. HealthTracker::record_failure(provider)
   c. Check retry policy: retries_remaining > 0?
   d. Router::select_excluding(failed_providers) → new SelectedProvider
   e. Send to new provider → success
   f. HealthTracker::record_success(new_provider)
   g. Return RequestOutcome { response, provider: new, attempt: 2, ... }
5-7. Same as happy path (but outcome notes retry)
```

#### Streaming Request (Happy Path)

```
1-3. Same as non-streaming
4. RequestExecutor::execute_stream(provider, request)
   a. Send to provider, get response stream
   b. Return raw byte stream + metadata channel
5. StreamWrapper::wrap(stream)
   a. Pass through each SSE chunk to client
   b. Parse "data: " lines to detect [DONE] marker
   c. If last chunk before [DONE] has usage field, extract token counts
   d. On stream end, send metadata (tokens, success) via oneshot channel
6. Handler returns wrapped stream as Body
7. When stream completes, metadata receiver fires:
   tokio::spawn(storage.log_request(metadata))
```

#### Streaming Request (Mid-Stream Error)

```
1-5a. Same as streaming happy path
5b. Provider disconnects mid-stream (no [DONE] received)
   StreamWrapper detects:
   - bytes_stream returns Err(e) on next poll
   - OR stream ends without [DONE] marker
5c. StreamWrapper sends error event to client:
   "data: {\"error\": {\"message\": \"Provider disconnected\", ...}}\n\n"
5d. StreamWrapper sends metadata with success: false
6. Storage logs the failed stream request
   NOTE: No retry for streaming — partial content already sent to client
```

## Component Details

### RequestExecutor

**Purpose:** Encapsulate the "try provider, handle failure, maybe retry" logic that is currently inline in the handler.

**Why a separate component (not inline in handler):**
- The handler currently mixes HTTP response building with provider communication
- Retry logic adds complexity (tracking attempts, excluding providers, different error types)
- Testability: can unit test retry behavior with mock HTTP client without spinning up axum

**Interface:**

```rust
pub struct RequestExecutor {
    http_client: Client,
    health: Arc<HealthTracker>,
}

pub struct RequestOutcome {
    pub response: serde_json::Value,  // parsed response body
    pub provider: SelectedProvider,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub latency: Duration,
    pub attempts: u32,
    pub success: bool,
}

impl RequestExecutor {
    /// Execute a non-streaming request with retry on failure.
    pub async fn execute(
        &self,
        providers: &[SelectedProvider],  // ordered: primary, then fallbacks
        request: &ChatCompletionRequest,
    ) -> Result<RequestOutcome, Error>;

    /// Execute a streaming request (no retry — content already sent).
    pub async fn execute_stream(
        &self,
        provider: &SelectedProvider,
        request: &ChatCompletionRequest,
    ) -> Result<(impl Stream<Item = Result<Bytes>>, oneshot::Receiver<StreamMetadata>), Error>;
}
```

**Retry policy:**
- Retry on: connection refused, timeout, HTTP 502/503/429
- Do NOT retry on: HTTP 400 (bad request), 401 (auth), 404 (model not found)
- Max attempts: 2 (original + 1 retry) — configurable in config.toml
- On retry: exclude failed provider, ask Router for next-best
- Between retries: no delay for non-streaming (latency matters more than back-pressure for single user)

**Key design decision: providers list, not single provider.**
The handler calls `Router::select()` to get the primary provider, then `Router::fallbacks()` for alternatives. These are passed together to the executor. This keeps the Router as the authority on provider ordering while letting the executor handle the try/retry mechanics.

### StreamWrapper

**Purpose:** Inspect SSE byte stream to detect errors and extract metadata without buffering the whole response.

**Why needed:**
- Current code pipes bytes_stream directly — errors are silent
- Token usage often appears in the final SSE chunk (OpenAI pattern: last chunk before `[DONE]` has `usage` field)
- Client needs a clean error signal if provider drops mid-stream

**Interface:**

```rust
pub struct StreamMetadata {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub success: bool,
    pub error: Option<String>,
}

pub fn wrap_sse_stream(
    stream: impl Stream<Item = Result<Bytes, reqwest::Error>>,
) -> (impl Stream<Item = Result<Bytes, std::io::Error>>, oneshot::Receiver<StreamMetadata>);
```

**Implementation approach:**
- Use `futures::stream::unfold` or manual `Stream` impl
- Buffer partial SSE lines (SSE events can be split across byte chunks)
- Parse each complete `data: {...}` line as JSON
- Look for `usage` field in parsed chunks
- On stream error: emit `data: {"error": ...}\n\n` then close
- On clean `data: [DONE]`: send metadata via oneshot channel

**Key design decision: no retry for streaming.**
Once streaming starts, partial content has been sent to the client. Retrying would result in duplicate partial content. The correct behavior is: signal the error and let the client retry the full request if needed. This matches how OpenAI SDKs handle streaming failures.

### Storage

**Purpose:** SQLite request logging with async writes that never block the response path.

**Interface:**

```rust
pub struct Storage {
    pool: sqlx::SqlitePool,
}

impl Storage {
    pub async fn new(database_url: &str) -> Result<Self>;
    pub async fn run_migrations(&self) -> Result<()>;

    /// Log a completed request. Fire-and-forget from handler.
    pub async fn log_request(&self, record: RequestRecord) -> Result<()>;

    /// Query total cost over time period.
    pub async fn total_cost(&self, since: Option<DateTime<Utc>>) -> Result<CostSummary>;

    /// Query cost breakdown by model.
    pub async fn cost_by_model(&self, since: Option<DateTime<Utc>>) -> Result<Vec<ModelCost>>;
}

pub struct RequestRecord {
    pub timestamp: DateTime<Utc>,
    pub request_id: Uuid,
    pub policy: Option<String>,
    pub model: String,
    pub provider: String,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cost_sats: Option<u64>,
    pub latency_ms: u64,
    pub attempts: u32,
    pub success: bool,
    pub stream: bool,
}
```

**Schema (from CLAUDE.md, already designed):**

```sql
CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    request_id TEXT NOT NULL,
    policy TEXT,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_sats INTEGER,
    latency_ms INTEGER,
    attempts INTEGER DEFAULT 1,
    stream BOOLEAN DEFAULT FALSE,
    success BOOLEAN
);

CREATE INDEX idx_requests_timestamp ON requests(timestamp);
CREATE INDEX idx_requests_model ON requests(model);
```

**Key design decisions:**

1. **Fire-and-forget logging:** `tokio::spawn(storage.log_request(record))` from the handler. Never block the response. If the write fails, log the error via tracing but do not propagate to client.

2. **SQLite WAL mode:** Enable Write-Ahead Logging on pool creation for better concurrent read/write performance. Single writer is fine for single-user proxy.

3. **Pool, not single connection:** Use `SqlitePool` with max 1 writer + multiple readers. sqlx handles connection pooling.

4. **Migrations at startup:** `Storage::new()` runs `CREATE TABLE IF NOT EXISTS` on first connection. No external migration tool needed for this simple schema.

### HealthTracker

**Purpose:** Track which providers are healthy so the Router can deprioritize failing ones and the executor can skip known-bad providers.

**Interface:**

```rust
pub struct HealthTracker {
    states: DashMap<String, ProviderHealth>,
}

pub struct ProviderHealth {
    pub consecutive_failures: u32,
    pub last_success: Option<Instant>,
    pub last_failure: Option<Instant>,
    pub is_healthy: bool,
}

impl HealthTracker {
    pub fn record_success(&self, provider: &str);
    pub fn record_failure(&self, provider: &str);
    pub fn is_healthy(&self, provider: &str) -> bool;
    pub fn all_health(&self) -> Vec<(String, ProviderHealth)>;
}
```

**Health logic:**
- Provider marked unhealthy after N consecutive failures (default: 3)
- Provider automatically retried after cooldown period (default: 60 seconds)
- Success resets consecutive failure count
- Healthy by default (no data = assume healthy)

**Key design decision: in-memory only, not persisted.**
Health state is transient. On restart, all providers start healthy. This is correct because provider health changes rapidly and stale data from hours ago is misleading. The DashMap provides lock-free concurrent access from multiple handler tasks.

**Dependency note:** DashMap is not currently in Cargo.toml. Alternative: use `tokio::sync::RwLock<HashMap<String, ProviderHealth>>` to avoid a new dependency. The RwLock approach is fine for the expected provider count (2-10 providers, contention negligible).

### CostCalculator

**Purpose:** Compute actual request cost from token counts and provider rates.

**Interface:**

```rust
pub fn calculate_cost(
    provider: &SelectedProvider,
    input_tokens: u32,
    output_tokens: u32,
) -> u64;  // cost in sats
```

**Formula (from PROJECT.md):**
```
cost_sats = (input_tokens * input_rate / 1000) + (output_tokens * output_rate / 1000) + base_fee
```

**This fixes the current bug** where only `output_rate` is used for selection. The cost calculator is used:
1. By Storage when logging (actual cost from actual tokens)
2. Optionally by Router when selecting (estimated cost from estimated tokens)

For selection, the Router can continue using output_rate as a proxy (it correlates well with total cost) or switch to estimated total cost if token estimation is added later.

## Suggested Build Order

Build order is constrained by dependencies between components:

```
Phase 1: Foundation (no inter-component dependencies)
  ├── Storage (standalone, just needs sqlx + schema)
  ├── CostCalculator (pure function, no deps)
  └── RequestExecutor skeleton (extract from handler, no retry yet)

Phase 2: Wiring (components connect)
  ├── Wire Storage into handler (tokio::spawn log after response)
  ├── Wire CostCalculator into handler (compute before logging)
  └── Add cost query endpoint (/stats or /costs)

Phase 3: Reliability (builds on executor)
  ├── HealthTracker (in-memory, simple)
  ├── RequestExecutor retry logic (uses HealthTracker)
  └── StreamWrapper (error detection, metadata extraction)

Phase 4: Polish
  ├── Wire HealthTracker into Router (deprioritize unhealthy)
  ├── Config for retry policy (max_retries, retry_on codes)
  └── /health endpoint enhanced with provider health
```

**Rationale for this order:**

1. **Storage first** because it has zero dependencies on other new components. It provides immediate value (you can see what happened). It also validates that sqlx + SQLite works correctly in the existing async setup before adding complexity.

2. **CostCalculator alongside Storage** because it is a pure function that is trivial to implement and test, and Storage needs it for the `cost_sats` field.

3. **RequestExecutor extraction before retry** because the handler refactor (moving provider-calling code from `chat_completions` into a separate struct) is a prerequisite for adding retry logic. Doing the extraction as a pure refactor (same behavior) makes it safe and reviewable.

4. **Reliability after observability** because you need to see failures before you can handle them well. With logging in place, you can observe: how often do providers fail? What errors? This data informs retry policy tuning. Without logging, you are tuning retry policy blind.

5. **StreamWrapper last in reliability** because it is the most complex component (SSE parsing, partial buffer management, error injection) and provides the least common benefit (mid-stream failures are rare compared to connection/timeout failures that the executor handles).

## Patterns to Follow

### Pattern 1: Fire-and-Forget Async Logging

**What:** Log request outcomes without blocking the response path.

**When:** After every request, both streaming and non-streaming.

**Why this pattern:** The client should not wait for a database write. If logging fails, the request still succeeded from the client's perspective.

```rust
// In handler, after building response:
let storage = state.db.clone();
tokio::spawn(async move {
    if let Err(e) = storage.log_request(record).await {
        tracing::error!(error = %e, "Failed to log request");
    }
});
```

**Confidence:** HIGH. This is standard Tokio practice for non-critical async side effects. The `tokio::spawn` moves the work to the Tokio runtime's task pool. The `clone()` on Arc<Storage> is cheap.

### Pattern 2: Ordered Provider List for Retry

**What:** Router returns a ranked list of providers. Executor tries them in order.

**When:** Non-streaming requests where the full response has not been sent yet.

```rust
// Router provides ordered candidates
let candidates = state.router.select_with_fallbacks(&request.model, policy, prompt)?;
// candidates: [cheapest, next_cheapest, ...]

// Executor tries in order
let outcome = executor.execute(&candidates, &request).await?;
```

**Why this pattern (not Tower retry middleware):**
- Tower's `retry` layer works at the Service level and retries the same request to the same endpoint
- arbstr needs to retry to a DIFFERENT provider, which requires re-running provider selection
- The retry decision depends on the error type (retry 502, don't retry 400)
- Custom retry is simpler here than fighting Tower's abstractions

**Confidence:** HIGH. Tower retry is for retrying the same operation. Provider fallback is a different pattern (try alternative) that is better expressed as application logic.

### Pattern 3: Oneshot Channel for Stream Metadata

**What:** Streaming responses use a `tokio::sync::oneshot` channel to send metadata back to the handler after the stream completes.

**When:** Streaming requests where token counts are only known after the last chunk.

```rust
let (tx, rx) = tokio::sync::oneshot::channel::<StreamMetadata>();

let wrapped_stream = stream.map(move |chunk| {
    // ... process chunk, detect [DONE], extract usage ...
    // On stream end:
    let _ = tx.send(metadata);
    chunk
});

// After building response with wrapped_stream body:
tokio::spawn(async move {
    if let Ok(metadata) = rx.await {
        storage.log_request(metadata.into()).await.ok();
    }
});
```

**Confidence:** MEDIUM. This is a clean pattern but the implementation has a subtlety: the `tx.send()` inside the stream closure requires careful ownership. The `tx` must be moved into the stream's final callback. This is achievable with `futures::stream::unfold` or by wrapping tx in an `Option` inside a stateful stream adapter.

### Pattern 4: Extract-Then-Enhance Refactoring

**What:** Extract the provider-calling code from the handler into `RequestExecutor` as a pure refactor (same behavior), then add retry logic to the executor.

**When:** When adding retry/fallback to existing working code.

**Why:** Two small changes are safer than one big change. The extraction can be tested (does the proxy still work identically?). Then retry logic is added to a clean, focused component.

**Confidence:** HIGH. Standard software engineering practice.

## Anti-Patterns to Avoid

### Anti-Pattern 1: Blocking Database Writes in Request Path

**What:** Calling `storage.log_request().await` before returning the response.

**Why bad:** Adds 1-5ms of latency to every request for a write the client doesn't need. If SQLite is slow (lock contention, disk flush), requests back up.

**Instead:** `tokio::spawn` the write. Accept that a crash between response and write loses one log entry. For a personal proxy, this tradeoff is correct.

### Anti-Pattern 2: Retrying Streaming Requests

**What:** Attempting to retry a streaming request after some chunks have already been sent to the client.

**Why bad:** The client has already received partial content. Retrying would send content from a new completion, resulting in incoherent output (first half from provider A, second half from provider B, different completions).

**Instead:** For streaming, signal the error in-band (send an error SSE event) and let the client decide whether to retry the full request. Most OpenAI SDKs handle this automatically.

### Anti-Pattern 3: Tower Retry for Provider Fallback

**What:** Using `tower::retry::Retry` middleware to implement provider fallback.

**Why bad:** Tower retry retries the same request to the same service. Provider fallback needs to select a DIFFERENT provider and build a DIFFERENT upstream request (different URL, different auth). Forcing this into Tower's retry abstraction requires a complex custom `Policy` that reaches back into the Router, creating circular dependencies.

**Instead:** Implement retry as a loop in `RequestExecutor::execute()`. Simple, explicit, testable.

### Anti-Pattern 4: Shared Mutable State via Mutex in Hot Path

**What:** Using `Arc<Mutex<HashMap>>` for health tracking when multiple request handlers need concurrent access.

**Why bad:** Mutex contention on every request. Even with short critical sections, under load (many concurrent requests), threads wait.

**Instead:** Use `tokio::sync::RwLock` (many concurrent readers, single writer) or `DashMap` (lock-free reads). For arbstr's scale (single user, <10 providers), even `RwLock<HashMap>` is fine, but it's good practice.

### Anti-Pattern 5: Parsing Every SSE Chunk as Full JSON

**What:** Attempting to deserialize every SSE `data:` line as a full `ChatCompletionChunk` struct.

**Why bad:** Adds overhead and fragility. If the provider sends a chunk that doesn't match the exact struct definition, the stream breaks.

**Instead:** Use `serde_json::Value` for lightweight inspection. Only look for the `usage` field in chunks. Pass bytes through unmodified to the client.

## Integration Points with Existing Code

### AppState Changes

```rust
// src/proxy/server.rs - add new fields
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: Arc<Storage>,           // NEW
    pub health: Arc<HealthTracker>, // NEW
    pub executor: RequestExecutor,  // NEW
}
```

Storage and HealthTracker are initialized in `run_server()` before creating AppState. The executor is created from the http_client and health tracker.

### Handler Changes

The `chat_completions` handler in `src/proxy/handlers.rs` changes from:
- Directly building and sending the upstream request (lines 56-85)
- Directly piping the stream (lines 87-104)

To:
- Calling `executor.execute()` or `executor.execute_stream()`
- Building the response from the `RequestOutcome`
- Spawning the logging task

The handler stays as the orchestrator but delegates the messy parts.

### Router Changes (Minimal)

The Router gains one new method:

```rust
impl Router {
    // Existing
    pub fn select(...) -> Result<SelectedProvider>;

    // New: return multiple candidates for fallback
    pub fn select_with_fallbacks(
        &self,
        model: &str,
        policy_name: Option<&str>,
        prompt: Option<&str>,
        exclude: &[String],  // provider names to skip
    ) -> Result<Vec<SelectedProvider>>;
}
```

This is a small extension of the existing `select()` logic: instead of returning the single cheapest, return all candidates sorted by cost. The executor takes the first, and on failure, moves to the next.

### Config Changes

```toml
# New section in config.toml
[reliability]
max_retries = 1          # 0 = no retry, 1 = one retry (2 total attempts)
retry_on_timeout = true
retry_on_server_error = true  # 502, 503

[database]
path = "./arbstr.db"
```

## Scalability Considerations

| Concern | Current (single user) | At 10 concurrent requests | At 100 concurrent requests |
|---------|-----------------------|---------------------------|----------------------------|
| SQLite writes | Fine, WAL mode | Fine, <1ms writes | May need write batching |
| Health tracking | RwLock fine | RwLock fine | Consider DashMap |
| Stream wrapping | Per-request allocation | Fine | Fine (each stream independent) |
| Memory per request | ~50KB (request + response) | ~500KB total | ~5MB total, fine |

For arbstr's stated use case (single user on home network), all components are well within capacity. The architecture supports growth to multi-user without fundamental changes — Storage already logs per-request, HealthTracker is concurrent-safe, and the executor is stateless per-request.

## Sources and Confidence

| Finding | Confidence | Basis |
|---------|------------|-------|
| Fire-and-forget tokio::spawn for logging | HIGH | Standard Tokio pattern, used widely in axum applications |
| Tower retry not suitable for provider fallback | HIGH | Based on Tower's Retry design (retries same service), verified by reading tower 0.4 API |
| SQLite WAL mode for concurrent access | HIGH | Well-documented SQLite feature, sqlx supports it via PRAGMA |
| Oneshot channel for stream metadata | MEDIUM | Sound pattern, implementation subtlety around ownership in stream closures |
| DashMap vs RwLock for health tracking | MEDIUM | Both work; DashMap adds a dependency, RwLock is sufficient at this scale |
| SSE error injection (send error event on mid-stream failure) | MEDIUM | OpenAI SDKs handle error events, but exact parsing behavior not verified against all clients |
| No retry for streaming requests | HIGH | Fundamental constraint: partial content already sent, cannot un-send |

## Open Questions

1. **SSE parsing robustness:** How exactly do different OpenAI client SDKs handle an error event mid-stream? Claude Code, Cursor, and the Python OpenAI SDK may differ. Testing with actual clients is needed during implementation.

2. **Token usage in streaming:** Does api.routstr.com include `usage` in the final SSE chunk? The OpenAI API optionally includes it (with `stream_options: {"include_usage": true}`). If Routstr does not support this, token counts for streaming requests will be null and cost tracking will be incomplete.

3. **Retry idempotency:** Chat completions are non-idempotent (each call produces different output). Retrying is safe in the sense that the client gets a valid response, but the retried response may differ from what the first attempt would have returned. This is acceptable for LLM requests.

4. **HealthTracker persistence across restarts:** Currently designed as in-memory only. If arbstr restarts frequently, providers with intermittent issues will always get a "clean slate." This is intentional but worth noting.

---

*Architecture research: 2026-02-02*
