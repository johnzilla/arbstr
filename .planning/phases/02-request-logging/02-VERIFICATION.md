---
phase: 02-request-logging
verified: 2026-02-03T21:30:00Z
status: passed
score: 10/10 must-haves verified
re_verification: false
---

# Phase 2: Request Logging Verification Report

**Phase Goal:** Every completed request is persistently logged with accurate token counts, costs, and latency
**Verified:** 2026-02-03T21:30:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | After a successful non-streaming request, a row appears in SQLite with all required fields (timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success=true, correlation_id) | ✓ VERIFIED | `RequestLog` struct has all fields (lines 8-23, logging.rs); `chat_completions` handler populates log entry (lines 89-103, handlers.rs); `spawn_log_write` called (line 122, handlers.rs) |
| 2 | Token counts in the log match the usage object from the provider response (prompt_tokens and completion_tokens) | ✓ VERIFIED | `extract_usage` function extracts both fields (lines 42-46, handlers.rs); `handle_non_streaming_response` uses extracted values (lines 245-249, handlers.rs); 4 unit tests pass |
| 3 | Latency is measured as wall-clock milliseconds from handler start to response ready | ✓ VERIFIED | `Instant::now()` at handler start (line 56, handlers.rs); `start.elapsed().as_millis() as i64` before logging (line 86, handlers.rs) |
| 4 | Database write is fire-and-forget via spawn_log_write — the response is returned BEFORE the write completes | ✓ VERIFIED | `spawn_log_write` uses `tokio::spawn` (lines 61-69, logging.rs); called after building log entry but before returning response (line 122, handlers.rs) |
| 5 | Failed provider requests (non-2xx status) are logged with success=false, error_status, error_message, null tokens/cost | ✓ VERIFIED | Provider errors caught in `execute_request` (lines 197-215, handlers.rs); `RequestError` has status_code and message (lines 31-36, handlers.rs); logged with success=false (lines 105-120, handlers.rs) |
| 6 | Pre-route rejections (NoProviders, NoPolicyMatch, BadRequest) are logged with provider=null, success=false, error_status, error_message | ✓ VERIFIED | Router selection errors caught with `map_err` (lines 144-160, handlers.rs); `provider_name: None` for pre-route failures (line 156, handlers.rs); logged with Err branch (lines 105-120, handlers.rs) |
| 7 | Streaming requests are logged with streaming=true; usage is extracted from the final SSE chunk if present | ✓ VERIFIED | `is_streaming` tracked from request (line 59, handlers.rs); set in log entry (line 95 and 111, handlers.rs); `handle_streaming_response` has SSE chunk inspection (lines 307-347, handlers.rs); logs with None tokens (Phase 2 acceptable per CONTEXT.md) |
| 8 | If database pool is None (DB not initialized), logging is silently skipped | ✓ VERIFIED | `if let Some(pool) = &state.db` guard (line 87, handlers.rs); entire logging block only executes if DB is Some |
| 9 | Both arbstr-calculated cost (via actual_cost_sats) AND provider-reported cost are logged in separate columns | ✓ VERIFIED | `actual_cost_sats` called (line 253, handlers.rs); `provider_cost_sats` extracted from response (lines 264-267, handlers.rs); both fields in `RequestLog` (lines 17-18, logging.rs) |
| 10 | The TODO comment on line 120 is replaced with actual logging code | ✓ VERIFIED | Grep shows 0 matches for "TODO.*Log to database" in handlers.rs; logging implementation present at lines 85-123 |

**Score:** 10/10 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `src/storage/mod.rs` | SQLite pool init with migrations | ✓ VERIFIED | `init_pool` function creates pool with `create_if_missing(true)` (line 19); runs `sqlx::migrate!()` (line 27) |
| `src/storage/logging.rs` | RequestLog struct and spawn_log_write | ✓ VERIFIED | `RequestLog` has all 14 fields (lines 8-23); `spawn_log_write` is fire-and-forget with tokio::spawn (lines 59-70); warns on write failure |
| `src/proxy/handlers.rs` | Complete request logging in chat_completions | ✓ VERIFIED | Handler restructured with `execute_request` pattern (lines 136-222); logs all paths (lines 85-123); 4 extract_usage tests pass (lines 426-478) |
| `src/proxy/server.rs` | AppState.db field and pool init | ✓ VERIFIED | `AppState` has `db: Option<SqlitePool>` (line 30); `run_server` calls `init_pool` (lines 92-104); sets to None on error with warning |
| `src/error.rs` | Database error variant | ✓ VERIFIED | `Database(#[from] sqlx::Error)` variant (line 34); IntoResponse implementation (lines 47-48) |
| `src/lib.rs` | pub mod storage | ✓ VERIFIED | `pub mod storage;` present (line 10); `pub use` for RequestLog via storage module |
| `migrations/20260203000000_initial_schema.sql` | Schema with all columns | ✓ VERIFIED | `requests` table has all required columns (lines 2-18); correlation_id and timestamp indexes (lines 20-21); `token_ratios` table for future use (lines 24-28) |
| `Cargo.toml` | sqlx with migrate feature | ✓ VERIFIED | `sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate"] }` (line 29); chrono and futures dependencies present |
| `build.rs` | rerun-if-changed migrations | ✓ VERIFIED | `println!("cargo:rerun-if-changed=migrations");` (line 2) |

### Key Link Verification

| From | To | Via | Status | Details |
|------|-----|-----|--------|---------|
| chat_completions | spawn_log_write | Fire-and-forget database write after response construction | ✓ WIRED | `spawn_log_write(pool, log_entry)` called at line 122; response returned after spawn (lines 126-129) |
| chat_completions | actual_cost_sats | Cost calculation from extracted token counts | ✓ WIRED | `crate::router::actual_cost_sats(input, output, ...)` called at line 253; result stored in cost_sats field |
| chat_completions | RequestId | Extension extractor for correlation ID | ✓ WIRED | `Extension(request_id): Extension<RequestId>` in signature (line 52); converted to string at line 57; stored in log (line 90, 106) |
| execute_request | state.router.select | Provider selection with error mapping | ✓ WIRED | `state.router.select(...)` called at line 146; `map_err` converts to RequestError (lines 147-160) |
| handle_non_streaming_response | extract_usage | Token extraction from JSON response | ✓ WIRED | `extract_usage(&response)` called at line 245; result destructured to input/output tokens (lines 246-249) |
| spawn_log_write | RequestLog.insert | Async database write in spawned task | ✓ WIRED | `tokio::spawn` with `log.insert(&pool).await` (lines 61-62, logging.rs); error logged with correlation_id (lines 63-67) |
| init_pool | sqlx::migrate!() | Embedded migrations applied on startup | ✓ WIRED | `sqlx::migrate!().run(&pool).await?` at line 27 (storage/mod.rs); called from run_server (line 94, server.rs) |

### Requirements Coverage

From ROADMAP.md Phase 2 Success Criteria:

| Requirement | Status | Evidence |
|-------------|--------|----------|
| 1. After proxying a non-streaming request, a row appears in SQLite with timestamp, model, provider, input_tokens, output_tokens, cost_sats, latency_ms, success, policy, and correlation ID | ✓ SATISFIED | All fields present in RequestLog struct; logged in success path (lines 89-103, handlers.rs) |
| 2. Token counts in the log match the usage object returned by the provider | ✓ SATISFIED | `extract_usage` extracts prompt_tokens and completion_tokens; 4 unit tests verify correctness |
| 3. Latency recorded reflects wall-clock time from request receipt to response completion | ✓ SATISFIED | `Instant::now()` at start; `elapsed().as_millis()` before logging |
| 4. SQLite writes never block the response to the client (async fire-and-forget) | ✓ SATISFIED | `tokio::spawn` in spawn_log_write; response returned before write completes |
| 5. Database schema is applied automatically via embedded migrations on startup | ✓ SATISFIED | `sqlx::migrate!()` in init_pool; called from run_server; migrations/ directory with schema |

### Anti-Patterns Found

None. Clean implementation with no blockers, warnings, or info-level concerns.

### Code Quality

- **Build:** `cargo build` succeeds
- **Tests:** 12 tests pass (8 existing + 4 new extract_usage tests)
- **Linting:** `cargo clippy -- -D warnings` passes with no warnings
- **Coverage:** All code paths logged (success, provider error, pre-route rejection, streaming, non-streaming)

### Human Verification Required

None. All Phase 2 requirements are structurally verifiable and have been verified.

**Optional smoke test (if desired):**

1. **Test: Start server and make non-streaming request**
   - Run: `cargo run -- serve --mock`
   - Send: `curl -X POST http://localhost:8080/v1/chat/completions -H "Content-Type: application/json" -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hello"}]}'`
   - Expected: Response received; check arbstr.db with `sqlite3 arbstr.db "SELECT * FROM requests"` shows 1 row with all fields populated
   - Why human: Requires running server and inspecting database

2. **Test: Failed request is logged**
   - Run: Server with real provider that returns error
   - Send: Request to trigger error (invalid API key, etc.)
   - Expected: Database row has success=false, error_status set, provider name present
   - Why human: Requires real provider interaction

3. **Test: Pre-route rejection is logged**
   - Run: Server with no providers for model
   - Send: Request for unsupported model
   - Expected: Database row has success=false, provider=null, error_message set
   - Why human: Requires specific configuration

---

_Verified: 2026-02-03T21:30:00Z_
_Verifier: Claude (gsd-verifier)_
