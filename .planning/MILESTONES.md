# Project Milestones: arbstr

## v1 Reliability and Observability (Shipped: 2026-02-04)

**Delivered:** Added reliability (retry with fallback) and observability (SQLite logging, response metadata headers) to the existing proxy, with corrected cost calculation as foundation.

**Phases completed:** 1-4 (10 plans total)

**Key accomplishments:**

- Fixed cost calculation to use full formula with f64 sub-satoshi precision
- SQLite-backed request logging with async fire-and-forget writes and token extraction
- Per-request correlation IDs and response metadata headers (cost, latency, request-id, provider)
- Retry with exponential backoff and fallback to next cheapest provider
- OpenAI-compatible error responses through all retry/fallback/timeout paths
- 33 automated tests with zero clippy warnings

**Stats:**

- 70 files in repository
- 2,840 lines of Rust
- 4 phases, 10 plans
- 12 days from project start to ship (2026-01-23 → 2026-02-04)

**Git range:** `30dd6c2` (initial) → `05b46f9` (docs(04): complete retry and fallback phase)

**What's next:** v2 requirements — cost query endpoints, streaming token extraction, circuit breaker, stream error handling

---

## v1.1 Secrets Hardening (Shipped: 2026-02-15)

**Delivered:** Eliminated plaintext API keys from config files and all output surfaces. Keys are now protected by the Rust type system, loadable from environment variables, and never exposed in logs, endpoints, or CLI output.

**Phases completed:** 5-7 (4 plans, 8 tasks)

**Key accomplishments:**

- ApiKey newtype wrapping SecretString with Debug/Display/Serialize redaction and zeroize-on-drop
- `${VAR}` expansion engine with convention-based `ARBSTR_<NAME>_API_KEY` auto-discovery
- Per-provider key source logging at startup and key availability reporting in check command
- File permission warnings for overly permissive config files (Unix)
- Masked key prefixes (`cashuA...***`) in /providers endpoint and providers CLI
- Plaintext literal key warnings with actionable env var suggestions
- 69 automated tests (41 existing + 28 new), zero clippy warnings

**Stats:**

- 3,892 lines of Rust
- 3 phases, 4 plans, 8 tasks
- 23 files changed (4,114 insertions, 135 deletions)
- 1 day from start to ship (2026-02-15)

**Git range:** `2764ded` (feat(05-01)) → `0aa7a97` (docs(phase-07): complete)

**What's next:** Planning next milestone

---


## v1.2 Streaming Observability (Shipped: 2026-02-16)

**Delivered:** Complete streaming observability — every streaming request now logs accurate token counts, cost, full-duration latency, and completion status, with cost surfaced to clients via trailing SSE event.

**Phases completed:** 8-10 (3 phases, 4 plans, 7 tasks)

**Key accomplishments:**

- StreamOptions injection ensuring providers send usage data in final SSE chunk
- SseObserver line-buffered SSE parser with cross-chunk boundary reassembly and usage extraction
- wrap_sse_stream API with panic isolation (catch_unwind) and Drop-based result finalization
- Channel-based streaming handler (mpsc) with background task for post-stream observability
- Trailing SSE event with arbstr metadata (cost_sats, latency_ms) after upstream [DONE]
- Post-stream DB UPDATE for tokens, cost, stream_duration_ms, and completion status
- 94 automated tests (85 existing + 9 new), zero clippy warnings

**Stats:**

- ~5,000 lines of Rust
- 3 phases, 4 plans, 7 tasks
- 23 files changed (+4,122 / -59 lines)
- 1 day from start to ship (2026-02-16)

**Git range:** `4e44628` (feat(08-01)) → `16dd554` (feat(10-01))

**What's next:** Planning next milestone

---


## v1.3 Cost Querying API (Shipped: 2026-02-16)

**Delivered:** Read-only API endpoints exposing cost and performance data from SQLite logs — aggregate stats with time range presets and model/provider filtering, plus paginated request log browsing with sorting.

**Phases completed:** 11-12 (2 phases, 4 plans, 8 tasks)

**Key accomplishments:**

- GET /v1/stats endpoint with aggregate cost/performance queries and per-model breakdown
- Read-only SQLite connection pool (max 3) isolating analytics from proxy writes
- Time range presets (last_1h, last_24h, last_7d, last_30d) and ISO 8601 since/until filtering
- GET /v1/requests endpoint with paginated request log listing (page-based, max 100 per page)
- Nested response structure (tokens/cost/timing/error sections) with curated field set
- Sort by timestamp, cost, or latency with column name whitelist for SQL injection prevention
- 34 new integration tests (14 stats + 20 logs), 137 total automated tests, zero clippy warnings

**Stats:**

- ~6,000 lines of Rust
- 2 phases, 4 plans, 8 tasks
- 29 files changed (+4,067 / -83 lines)
- 1 day from start to ship (2026-02-16)
- Zero new dependencies

**Git range:** `46a7a16` (feat(11-01)) → `a57acaf` (docs(phase-12): complete)

**What's next:** Planning next milestone

---


## v1.4 Circuit Breaker (Shipped: 2026-02-16)

**Delivered:** Per-provider circuit breaker that stops sending to unhealthy providers, with automatic half-open recovery and enhanced /health reporting for operator visibility.

**Phases completed:** 13-15 (3 phases, 5 plans, 10 tasks)

**Key accomplishments:**

- Per-provider 3-state circuit breaker (Closed/Open/Half-Open) with DashMap-backed registry and consecutive failure tracking
- Queue-and-wait half-open recovery with ProbeGuard RAII for stuck-probe prevention
- Handler-level circuit filtering — skip open circuits before retry loop, 503 fail-fast when all providers down
- Streaming and non-streaming outcome recording for circuit state updates
- Enhanced /health endpoint with per-provider circuit state, failure counts, and computed ok/degraded/unhealthy status
- 46 new tests (16 unit + 21 integration + 9 circuit routing), 183 total automated tests, zero clippy warnings

**Stats:**

- 9,863 lines of Rust
- 3 phases, 5 plans, 10 tasks
- 26 files changed (+4,840 / -94 lines)
- 1 day from start to ship (2026-02-16)

**Git range:** `f58c00f` (feat(13-01)) → `55512bd` (docs(phase-15): complete)

**What's next:** Planning next milestone

---

