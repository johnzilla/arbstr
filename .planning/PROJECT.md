# arbstr

## What This Is

arbstr is a local proxy that sits between your applications and the Routstr decentralized AI marketplace. It provides an OpenAI-compatible API and selects the optimal model for each request based on cost and policy constraints, enabling sats-denominated model arbitrage through a single Routstr endpoint. It retries failed requests with exponential backoff and falls back to alternate providers, logs every request to SQLite with cost and latency tracking, and exposes per-request metadata via response headers. Built in Rust, it's designed for personal use today with architectural decisions that support future multi-user deployment.

## Core Value

Smart model selection that minimizes sats spent per request without sacrificing quality — pick the cheapest model that fits the task.

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

### Active

- [ ] Stream error handling (detect mid-stream failures, signal client cleanly)
- [ ] Basic cost query endpoint (total spend, per-model breakdown)
- [ ] Per-model and per-policy cost breakdown queries
- [ ] Enhanced /health endpoint with per-provider status and success rates
- [ ] Learned token ratios per policy (predict cost before seeing response)
- [ ] Token counts extracted from streaming responses (SSE parsing or stream_options)
- [ ] Circuit breaker per provider (stop sending after N consecutive failures)
- [ ] Per-provider timeout configuration (replace global 30s)

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
- **Shipped v1**: 2,840 lines Rust across ~15 source files. Working proxy with routing, policy engine, SQLite logging, response metadata headers, retry with fallback. 33 automated tests, clippy clean.
- **Known concerns**: Streaming token extraction not yet implemented (tokens logged as None), streaming errors are silent (no retry for streaming), Cashu token double-spend semantics during retry need verification.

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

---
*Last updated: 2026-02-04 after v1 milestone*
