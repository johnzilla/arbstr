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

