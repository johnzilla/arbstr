# arbstr

## What This Is

arbstr is a local proxy that sits between your applications and the Routstr decentralized AI marketplace. It provides an OpenAI-compatible API and selects the optimal model for each request based on cost and policy constraints, enabling sats-denominated model arbitrage through a single Routstr endpoint. It retries failed requests with exponential backoff and falls back to alternate providers, logs every request to SQLite with cost and latency tracking, and exposes per-request metadata via response headers. Built in Rust, it's designed for personal use today with architectural decisions that support future multi-user deployment.

## Core Value

Smart model selection that minimizes sats spent per request without sacrificing quality — pick the cheapest model that fits the task.

## Current Milestone

No active milestone. All planned features through v1.3 have shipped.

## Requirements

### Validated

- ✓ OpenAI-compatible proxy server (POST /v1/chat/completions, /v1/models, /health) — existing
- ✓ Multi-model provider configuration via TOML with sats-based pricing — existing
- ✓ Cheapest model selection based on advertised output rates — existing
- ✓ Policy-based constraints (allowed models, max cost per 1k output tokens) — existing
- ✓ Keyword-based heuristic policy matching on user prompts — existing
- ✓ Explicit policy selection via X-Arbstr-Policy header — existing
- ✓ Streaming and non-streaming response forwarding — existing
- ✓ Mock mode for testing without real API calls — existing
- ✓ CLI with serve, check, and providers commands — existing
- ✓ TOML configuration with validation — existing
- ✓ Cost calculation uses full formula (input + output rates + base fee) — v1
- ✓ Per-request correlation IDs for tracing — v1
- ✓ Request logging to SQLite (provider, model, tokens, cost, latency, success) — v1
- ✓ Token count extraction from non-streaming provider responses — v1
- ✓ Async fire-and-forget SQLite writes — v1
- ✓ Response metadata headers (x-arbstr-cost-sats, x-arbstr-latency-ms, x-arbstr-request-id) — v1
- ✓ Provider fallback on failure (retry with backoff, fallback to next cheapest) — v1
- ✓ Retry metadata in x-arbstr-retries header — v1
- ✓ OpenAI-compatible error responses through all retry/fallback paths — v1
- ✓ API key fields use SecretString wrapper with Debug/Display/Serialize redaction — v1.1
- ✓ Secret values zeroized in memory when dropped — v1.1
- ✓ `${VAR}` syntax expansion in config values — v1.1
- ✓ Clear error when referenced env var is not set — v1.1
- ✓ Convention-based `ARBSTR_<NAME>_API_KEY` auto-discovery — v1.1
- ✓ Startup logs report per-provider key source without revealing key — v1.1
- ✓ `check` command reports key availability per provider — v1.1
- ✓ Startup warns on config file permissions > 0600 (Unix) — v1.1
- ✓ No API key leaks in endpoints, CLI, errors, or tracing — v1.1
- ✓ Masked key prefix display (`cashuA...***`) in providers output — v1.1
- ✓ Startup warns on literal plaintext keys in config — v1.1
- ✓ Token counts extracted from streaming responses via stream_options injection — v1.2
- ✓ Post-stream database UPDATE with accurate token counts and cost — v1.2
- ✓ Full-stream duration latency tracking (stream_duration_ms) — v1.2
- ✓ Stream completion status (normal, client disconnect, incomplete) — v1.2
- ✓ Trailing SSE event with arbstr metadata (cost_sats, latency_ms) — v1.2
- ✓ Graceful degradation for providers without usage data — v1.2
- ✓ Aggregate stats endpoint (total spend, request count, tokens, latency, success rate) — v1.3
- ✓ Per-model stats breakdown endpoint — v1.3
- ✓ Time range filtering on stats endpoints (since/until and preset shortcuts) — v1.3
- ✓ Model and provider filtering on stats endpoints — v1.3
- ✓ Paginated request log listing with filtering and sorting — v1.3
- ✓ Read-only analytics pool isolated from proxy writes — v1.3

### Active

(None — start next milestone with `/gsd:new-milestone`)

### Future

- Stream error handling (detect mid-stream failures, signal client cleanly)
- Per-policy cost breakdown queries
- Enhanced /health endpoint with per-provider status and success rates
- Learned token ratios per policy (predict cost before seeing response)
- Circuit breaker per provider (stop sending after N consecutive failures)
- Per-provider timeout configuration (replace global 30s)

### Out of Scope

- Multi-provider routing across different Routstr nodes — single endpoint (api.routstr.com) is the current use case
- Web dashboard UI — query endpoints are sufficient, CLI or curl for now
- Client authentication to arbstr — running on home network, only user
- Rate limiting — single user, no abuse vector
- Cashu wallet management — balance monitored externally at the mint
- ML-based policy classification — keyword heuristics are sufficient for now
- Production deployment tooling (Docker, systemd) — runs locally
- Cross-model fallback — silently substituting cheaper model changes quality

## Context

- **Routstr marketplace**: Live decentralized AI inference protocol at api.routstr.com. OpenAI-compatible API, payments via Cashu tokens (Bitcoin eCash). Fund a session with sats, get an sk- API key, each request deducts cost based on token usage.
- **Cost formula**: `(input_tokens * input_price) + (output_tokens * output_price) + request_fee` — all in satoshis.
- **Current architecture**: Config models "multiple providers" with different URLs, but actual usage is one Routstr endpoint with multiple models at different price points. The multi-provider abstraction supports future flexibility.
- **Shipped v1**: Working proxy with routing, policy engine, SQLite logging, response metadata headers, retry with fallback.
- **Shipped v1.1**: API keys protected by SecretString type with zeroize-on-drop. Environment variable expansion (`${VAR}`) and convention-based auto-discovery (`ARBSTR_<NAME>_API_KEY`). File permission warnings, masked key prefixes, literal key warnings. 3,892 lines Rust, 69 automated tests, clippy clean.
- **Shipped v1.2**: Streaming observability — every streaming request now logs accurate token counts, cost, full-duration latency, and completion status. Clients receive trailing SSE event with arbstr cost/latency metadata. ~5,000 lines Rust, 94 automated tests, clippy clean.
- **Shipped v1.3**: Cost querying API — GET /v1/stats for aggregate cost/performance data with time range presets, model/provider filtering, per-model breakdown. GET /v1/requests for paginated request log browsing with filtering and sorting. Read-only analytics pool isolated from proxy writes. ~6,000 lines Rust, 137 automated tests, clippy clean.
- **Known concerns**: Streaming errors are silent (no retry for streaming), Cashu token double-spend semantics during retry need verification. Routstr provider stream_options support unknown — safe degradation (NULL usage) prevents regression.

## Constraints

- **Tech stack**: Rust with Tokio/axum — established, no reason to change
- **API compatibility**: Must remain OpenAI-compatible — this is how all clients (Claude Code, Cursor, SDKs) connect
- **Payment unit**: Satoshis (sats) — Bitcoin-native, matches Routstr pricing
- **Database**: SQLite — simple, local, no external dependencies. Schema applied via embedded migrations.
- **Single user**: Architectural decisions should support multi-user future but implementation targets single-user home network

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Single Routstr endpoint as primary use case | User connects to api.routstr.com, arbitrage is across models not providers | ✓ Good — multi-provider abstraction kept but complexity managed |
| Keep multi-provider config abstraction | Future flexibility for multiple Routstr nodes without refactor | ✓ Good — used for retry fallback in v1 |
| Reliability before observability | Requests need to work reliably before tracking costs matters | ✓ Good — foundation + logging shipped before retry |
| SQLite for request logging | Already chosen (sqlx dep exists), simple, local, schema designed | ✓ Good — WAL mode, embedded migrations, fire-and-forget writes |
| No web dashboard | CLI/curl queries sufficient for single user, avoids frontend complexity | — Pending |
| Routing heuristic uses output_rate + base_fee | Token counts unknown at selection time; full formula used post-response | ✓ Good — actual_cost_sats handles real cost |
| actual_cost_sats returns f64 | Sub-satoshi precision for cheap models and accurate aggregation | ✓ Good — 0.125 sats preserved |
| UUID v4 generated internally | arbstr controls correlation ID, not read from client headers | ✓ Good — consistent tracing |
| Fire-and-forget logging via tokio::spawn | Never blocks response path; warns on failure | ✓ Good — verified non-blocking |
| Error path returns Ok(response) with headers | IntoResponse trait cannot accept additional context | ✓ Good — enables header attachment on all paths |
| Streaming omits cost/latency headers | Values not known at header-send time | ✓ Good — x-arbstr-streaming flag signals this |
| Generic retry with HasStatusCode trait | Decouples retry logic from handler types for testability | ✓ Good — 11 unit tests in isolation |
| Arc<Mutex<Vec>> for timeout-safe tracking | Attempt history must survive async cancellation | ✓ Good — enables x-arbstr-retries on 504 |
| Streaming bypasses retry | Cannot replay stream body; fail fast is correct behavior | ✓ Good — stream error handling deferred to v2 |
| secrecy v0.10 for SecretString | Ecosystem standard, serde support, zeroize-on-drop | ✓ Good — clean integration with custom Deserialize |
| ApiKey wraps SecretString directly | No intermediate trait needed for simplicity | ✓ Good — expose_secret() grep-auditable (1 call site) |
| Two-phase config loading (Raw → expand → Secret) | Clean env var integration without touching existing parse_str | ✓ Good — backward compatible, testable with closures |
| No new crates for env expansion | stdlib std::env::var is sufficient for ${VAR} | ✓ Good — zero dependency bloat |
| Separate from_file_with_env entry point | Keep existing from_file/parse_str unchanged | ✓ Good — zero regressions on existing tests |
| 6-char prefix for masked_prefix() | Identifies cashuA tokens without revealing content | ✓ Good — keys < 10 chars fall back to [REDACTED] |
| check_file_permissions returns Option | Caller controls warning format (tracing vs println) | ✓ Good — clean separation of detection vs reporting |
| Merge semantics for stream_options | Only add include_usage when missing, preserve client false | ✓ Good — no surprising override of client intent |
| Clone-and-mutate injection at send time | Keep original request immutable through handler chain | ✓ Good — clean separation of concerns |
| Vec<u8> buffer for SSE parsing | Defer UTF-8 validation to per-complete-line processing | ✓ Good — safe cross-chunk handling |
| 64KB buffer cap with full drain | Prevent OOM from misbehaving providers | ✓ Good — bounded memory, observable overflow |
| Strict [DONE] requirement | No data returned without [DONE] — unreliable streams yield empty | ✓ Good — correctness over permissiveness |
| Panic isolation via catch_unwind | Extraction bugs must never break client stream | ✓ Good — zero-impact observation |
| mpsc channel-based body | Background task consumes upstream, relays via channel | ✓ Good — enables post-stream trailing event + DB update |
| Trailing SSE event after upstream [DONE] | arbstr metadata (cost_sats, latency_ms) visible to clients | ✓ Good — minimal payload, standard SSE format |
| Continue upstream on client disconnect | Extract usage for DB even when client gone | ✓ Good — complete observability regardless of client |
| Separate read-only SQLite pool for analytics | Prevent analytics queries from starving proxy writes | ✓ Good — max 3 connections, clean isolation |
| TOTAL() not SUM() for nullable cost columns | Returns 0.0 instead of NULL on empty result sets | ✓ Good — no null handling needed in response |
| Column name whitelist via match for sort/group_by | Prevent SQL injection through dynamic ORDER BY/GROUP BY | ✓ Good — &'static str guarantees safety |
| Default time range last_7d when no params | Bounded queries prevent full table scans | ✓ Good — consistent UX across stats and logs |
| Two-query pagination (COUNT + SELECT) | COUNT(*) OVER() returns 0 when OFFSET exceeds rows | ✓ Good — correct total on all pages |
| Zero new dependencies for v1.3 | Existing stack (axum, sqlx, chrono, serde) covers everything | ✓ Good — no dependency bloat |
| Nested response sections (tokens/cost/timing/error) | Group related fields, hide internal columns | ✓ Good — clean API surface |

---
*Last updated: 2026-02-16 after v1.3 milestone complete*
