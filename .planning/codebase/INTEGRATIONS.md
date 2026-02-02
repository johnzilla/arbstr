# External Integrations

**Analysis Date:** 2026-02-02

## APIs & External Services

**LLM Providers (Routstr Marketplace):**
- Multiple configurable providers via OpenAI-compatible API
  - SDK/Client: reqwest 0.12
  - Auth: Optional Bearer token via `api_key` config field
  - Connection: Configured via `[[providers]]` in TOML with URL and optional API key
  - Request forwarding: `POST {provider.url}/v1/chat/completions` with request body forwarded as-is

**Provider Selection:**
- Configurable per request based on:
  - Explicit policy header: `X-Arbstr-Policy` (custom header in `src/proxy/handlers.rs`)
  - Heuristic keyword matching on user prompt (implemented in `src/router/selector.rs`)
  - Model availability across configured providers
  - Cost constraints from policies

**Request/Response Protocol:**
- OpenAI-compatible chat completion API at `/v1/chat/completions`
- Supports streaming (`stream: true`) and non-streaming responses
- Forwards requests to selected provider with Provider's API key in Authorization header
- Streams response back to client for streaming requests
- Adds arbstr metadata headers to responses:
  - `x-arbstr-provider`: Name of selected provider
  - Adds `arbstr_provider` field to JSON response body

## Data Storage

**Databases:**
- SQLite via sqlx 0.8
  - Connection: File path configured in `[database]` section of TOML (default: `./arbstr.db`)
  - Support for in-memory database via `:memory:` path (used in mock mode)
  - Client: sqlx with tokio runtime

**Database Schema (Not Yet Implemented):**
```sql
CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    policy TEXT,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_sats INTEGER,
    latency_ms INTEGER,
    success BOOLEAN
);

CREATE TABLE token_ratios (
    policy TEXT PRIMARY KEY,
    avg_ratio REAL,
    sample_count INTEGER
);
```

**File Storage:**
- Configuration file: `config.toml` (TOML format)
- Example config: `config.example.toml`
- SQLite database file: Default `./arbstr.db`

**Caching:**
- Not explicitly implemented
- In-memory configuration caching via Arc<Config> in AppState at `src/proxy/server.rs`

## Authentication & Identity

**Auth Provider:**
- Custom per-provider via Cashu tokens or API keys
- Implementation: `X-Arbstr-Policy` header for policy selection in `src/proxy/handlers.rs`

**Provider Authorization:**
- Each provider can have optional `api_key` field in config
- Forwarded as Bearer token: `Authorization: Bearer {api_key}` in `src/proxy/handlers.rs` line 64
- Header: `header::AUTHORIZATION` from axum

## Monitoring & Observability

**Error Tracking:**
- Not implemented with external service
- Error types defined in `src/error.rs` with OpenAI-compatible response format
- HTTP error responses include message, type, and code fields

**Logs:**
- Structured logging via tracing framework
- Subscriber setup in `src/main.rs` with configurable levels
- Default level: `arbstr=info,tower_http=info`
- Log requests and provider selection decisions
- Tower HTTP middleware provides request tracing
- Database logging planned but not yet implemented (TODO comment at `src/proxy/handlers.rs` line 120)

## CI/CD & Deployment

**Hosting:**
- Not specified in codebase - intended for local deployment or custom hosting
- Configured to bind to local TCP address (default: `127.0.0.1:8080`)

**CI Pipeline:**
- Not yet implemented
- Build instructions in CLAUDE.md: `cargo build --release`
- Test command: `cargo test`

## Environment Configuration

**Required env vars:**
- None strictly required - all critical config via `config.toml`
- Optional: `RUST_LOG` for log level control
- Optional: `ARBSTR_CONFIG` for alternate config file path
- Optional: `DATABASE_URL` for database path override

**Secrets location:**
- Config file (committed to `.gitignore`):
  - `config.toml` contains API keys (should not be committed)
  - `config.example.toml` is template without secrets
- `.env` file location: Listed in `.gitignore` but not used by application
- API keys stored in config file `api_key` field under `[[providers]]`

## Webhooks & Callbacks

**Incoming:**
- `/v1/chat/completions` - Proxy endpoint for LLM requests
- `/v1/models` - List available models across all providers
- `/health` - Health check endpoint
- `/providers` - arbstr extension showing configured providers

**Outgoing:**
- POST requests to configured provider URLs at `{provider.url}/v1/chat/completions`
- No webhook callbacks or asynchronous notifications

## Request Flow with External APIs

**Chat Completion Request Flow** (implemented in `src/proxy/handlers.rs`):

1. Client sends `POST /v1/chat/completions` to arbstr
2. Extract optional `X-Arbstr-Policy` header
3. Call `router.select()` to choose provider based on:
   - Model name from request
   - Policy constraints
   - User prompt for heuristic matching (keyword-based in `src/router/selector.rs`)
4. Build upstream URL: `{provider.url}/v1/chat/completions`
5. Forward request with:
   - JSON body copied from client request
   - Bearer token if provider has `api_key` configured
   - Content-Type header
6. Handle response:
   - Streaming: Forward `bytes_stream()` back to client with `text/event-stream` content type
   - Non-streaming: Parse JSON, add `arbstr_provider` metadata, return as JSON
7. Log response metadata (currently planned, not implemented)

## Cost Tracking (Not Yet Implemented)

**Token Ratio Learning:**
- Planned feature to learn input/output token ratios per policy
- Would use database storage from `token_ratios` table
- Currently not implemented - rates are configured statically

**Cost Calculation:**
- Formula: `(input_tokens * input_rate + output_tokens * output_rate + base_fee) / 1000`
- All values in satoshis
- Used for provider selection when "cheapest" strategy selected

---

*Integration audit: 2026-02-02*
