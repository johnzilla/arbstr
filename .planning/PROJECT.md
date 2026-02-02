# arbstr

## What This Is

arbstr is a local proxy that sits between your applications and the Routstr decentralized AI marketplace. It provides an OpenAI-compatible API and selects the optimal model for each request based on cost and policy constraints, enabling sats-denominated model arbitrage through a single Routstr endpoint. Built in Rust, it's designed for personal use today with architectural decisions that support future multi-user deployment.

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

### Active

- [ ] Provider fallback on failure (retry request, optionally with different model)
- [ ] Stream error handling (detect mid-stream failures, signal client cleanly)
- [ ] Request logging to SQLite (provider, model, tokens, cost, latency, success)
- [ ] Fix cost calculation (use input_rate + output_rate + base_fee, not just output_rate)
- [ ] Token count extraction from provider responses for accurate cost tracking
- [ ] Basic cost query endpoint (total spend, per-model breakdown)
- [ ] Learned token ratios per policy (predict cost before seeing response)

### Out of Scope

- Multi-provider routing across different Routstr nodes — single endpoint (api.routstr.com) is the current use case
- Web dashboard UI — query endpoints are sufficient, CLI or curl for now
- Client authentication to arbstr — running on home network, only user
- Rate limiting — single user, no abuse vector
- Cashu wallet management — balance monitored externally at the mint
- ML-based policy classification — keyword heuristics are sufficient for now
- Production deployment tooling (Docker, systemd) — runs locally

## Context

- **Routstr marketplace**: Live decentralized AI inference protocol at api.routstr.com. OpenAI-compatible API, payments via Cashu tokens (Bitcoin eCash). Fund a session with sats, get an sk- API key, each request deducts cost based on token usage.
- **Cost formula**: `(input_tokens * input_price) + (output_tokens * output_price) + request_fee` — all in satoshis.
- **Current architecture mismatch**: Config models "multiple providers" with different URLs, but actual usage is one Routstr endpoint with multiple models at different price points. The multi-provider abstraction can stay for future flexibility but shouldn't drive complexity.
- **Existing codebase**: ~10 Rust source files, working proxy with routing and policy engine. SQLite dependency (sqlx) included but not yet used. Schema designed but not implemented.
- **Codebase concerns**: Cost calculation only uses output_rate (broken), streaming errors are silent, no request persistence, keyword matching is simplistic.

## Constraints

- **Tech stack**: Rust with Tokio/axum — established, no reason to change
- **API compatibility**: Must remain OpenAI-compatible — this is how all clients (Claude Code, Cursor, SDKs) connect
- **Payment unit**: Satoshis (sats) — Bitcoin-native, matches Routstr pricing
- **Database**: SQLite — simple, local, no external dependencies. Already chosen (sqlx in Cargo.toml)
- **Single user**: Architectural decisions should support multi-user future but implementation targets single-user home network

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Single Routstr endpoint as primary use case | User connects to api.routstr.com, arbitrage is across models not providers | -- Pending |
| Keep multi-provider config abstraction | Future flexibility for multiple Routstr nodes without refactor | -- Pending |
| Reliability before observability | Requests need to work reliably before tracking costs matters | -- Pending |
| SQLite for request logging | Already chosen (sqlx dep exists), simple, local, schema designed | -- Pending |
| No web dashboard | CLI/curl queries sufficient for single user, avoids frontend complexity | -- Pending |

---
*Last updated: 2026-02-02 after initialization*
