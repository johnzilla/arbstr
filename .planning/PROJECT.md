# arbstr

## What This Is

arbstr is an open-source inference marketplace that routes AI compute requests to the cheapest qualified provider and settles payment in bitcoin over Lightning. It combines a Rust routing engine (arbstr core) with a TypeScript treasury service (arbstr vault) into a single deployable stack (arbstr node). Providers include mesh-llm nodes, Routstr endpoints, Ollama instances, or any OpenAI-compatible API. No tokens — just sats.

## Core Value

Route inference to the cheapest qualified provider and settle in bitcoin — NiceHash for AI inference.

## Current Milestone: v2.0 Inference Marketplace Foundation

**Goal:** Wire arbstr core to arbstr vault for live billing, add mesh-llm as a provider type, ship arbstr-node deployment, and launch arbstr.com.

**Target features:**
- Live vault billing via reserve/settle/release
- mesh-llm as a first-class provider
- arbstr-node Docker Compose full-stack deployment
- arbstr.com landing page with marketplace positioning

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
- ✓ Per-provider circuit breaker in AppState (Closed/Open/Half-Open states) — v1.4
- ✓ Circuit opens after 3 consecutive failures — v1.4
- ✓ Half-open probe after 30s timeout, success closes circuit — v1.4
- ✓ Router skips open-circuit providers during selection — v1.4
- ✓ 503 fail-fast when all providers for a model have open circuits — v1.4
- ✓ Enhanced /health endpoint with per-provider circuit state and failure counts — v1.4
- ✓ Provider tier system (local/standard/frontier) with config field — v1.7
- ✓ Heuristic complexity scorer with 5 configurable weighted signals — v1.7
- ✓ Tier-aware routing with configurable thresholds — v1.7
- ✓ X-Arbstr-Complexity header override (high/medium/low) — v1.7
- ✓ Automatic one-way tier escalation on circuit break — v1.7
- ✓ Complexity score + tier in response headers and SSE metadata — v1.7
- ✓ Complexity score + tier columns in request log DB — v1.7
- ✓ Stats endpoint group_by=tier support — v1.7

### Active

- [ ] End-to-end vault billing — arbstr core calls vault /internal/reserve, /internal/settle, /internal/release with live agent accounts
- [ ] mesh-llm provider support — localhost:9337 as a first-class provider type with auto-discovery
- [ ] arbstr-node Docker Compose deployment — core + vault + LND + Cashu mint in one `docker compose up`
- [ ] arbstr.com landing page — marketplace positioning, anti-token manifesto, getting started guide

### Future

- Stream error handling (detect mid-stream failures, signal client cleanly)
- Per-policy cost breakdown queries
- Learned token ratios per policy (predict cost before seeing response)
- Per-provider timeout configuration (replace global 30s)
- Pubky DHT provider discovery
- Nostr relay bridge for provider availability
- L402 anonymous access tier
- Seller account type with Lightning withdrawal
- Cross-node federation
- Provider reputation via Pubky semantic tags

### Out of Scope

- Web dashboard UI — query endpoints are sufficient, CLI or curl for now
- ML-based policy classification — keyword heuristics are sufficient for now
- Cross-model fallback — silently substituting cheaper model changes quality
- Invented tokens / governance / staking — bitcoin is the only money
- Multi-mint Cashu support — single self-hosted mint sufficient for v2.0

## Context

- **Product vision**: NiceHash for AI inference — an open marketplace for buying and selling AI compute, settled in bitcoin. No tokens, no staking, no governance theater. PRD at ~/Downloads/arbstr-prd.md.
- **Three repos**: arbstr (core routing, Rust), arbstr-vault (treasury, TypeScript, github.com/johnzilla/arbstr-vault), arbstr-node (deployment, github.com/johnzilla/arbstr-node).
- **Vault integration**: arbstr core's vault.rs calls 3 endpoints on arbstr vault: POST /internal/reserve, POST /internal/settle, POST /internal/release. Both sides are built and use X-Internal-Token auth. Vault has RESERVE/RELEASE/PAYMENT ledger pattern with crash-safe async wallet calls.
- **mesh-llm**: Block's distributed P2P inference network at docs.anarchai.org. Exposes OpenAI-compatible API on localhost:9337. Provides compute pool but no economic/arbitrage layer — that's arbstr.
- **Routstr**: One provider source among many. Live at api.routstr.com, OpenAI-compatible, Cashu token payments.
- **Cost formula**: `(input_tokens * input_price) + (output_tokens * output_price) + request_fee` — all in satoshis.
- **Codebase**: ~10,000 lines Rust, 244 automated tests, clippy clean. 7 shipped milestones (v1 through v1.7).
- **Known concerns**: Streaming errors are silent (no retry for streaming). Multimodal MessageContent::as_str() drops text parts after first. No validation that complexity_threshold_low < complexity_threshold_high.

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
| DashMap for per-provider circuit state | Per-shard locking, no cross-provider contention | ✓ Good — lock-free reads for uncontended providers |
| std::sync::Mutex (not tokio::sync::Mutex) for inner state | No .await points in state transitions | ✓ Good — simpler, no async overhead |
| Handler-level circuit integration | Not router or middleware — handler has retry context | ✓ Good — filter before retry loop prevents storm amplification |
| Single-permit half-open model | probe_in_flight flag prevents burst during recovery | ✓ Good — controlled recovery, no thundering herd |
| Lazy Open→HalfOpen transitions | Checked on request, no background timer | ✓ Good — zero overhead when no traffic |
| Hardcoded circuit constants (threshold=3, timeout=30s) | Defer configurability to future milestone | ✓ Good — simplicity, can add config later |

---
## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd-transition`):
1. Requirements invalidated? → Move to Out of Scope with reason
2. Requirements validated? → Move to Validated with phase reference
3. New requirements emerged? → Add to Active
4. Decisions to log? → Add to Key Decisions
5. "What This Is" still accurate? → Update if drifted

**After each milestone** (via `/gsd-complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-04-09 after v2.0 milestone started*
