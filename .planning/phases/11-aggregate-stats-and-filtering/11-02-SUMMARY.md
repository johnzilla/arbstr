---
phase: 11-aggregate-stats-and-filtering
plan: 02
subsystem: testing
tags: [integration-tests, axum, sqlite, tower-oneshot, stats-api]

requires:
  - phase: 11-aggregate-stats-and-filtering
    plan: 01
    provides: GET /v1/stats endpoint with aggregate SQL, filtering, grouping, time ranges
provides:
  - 14 integration tests covering all /v1/stats endpoint behaviors
  - Test helpers for in-memory SQLite setup, seeded data, tower::oneshot requests
  - Verification of all 5 phase success criteria
affects: [future stats endpoints, Phase 12 request log tests]

tech-stack:
  added: [tower/util (dev), http (dev)]
  patterns: [tower::ServiceExt::oneshot for integration tests, in-memory SQLite for test isolation]

key-files:
  created:
    - tests/stats.rs
  modified:
    - Cargo.toml
    - src/proxy/mod.rs
    - src/storage/stats.rs

key-decisions:
  - "tower::ServiceExt::oneshot approach for HTTP tests (no TCP listener, simpler than server bind)"
  - "rfc3339z helper to produce Z-suffix timestamps (avoids + sign URL encoding issues)"
  - "Seed data designed to exercise boundary conditions: recent vs old, success vs fail, streaming vs non-streaming"

patterns-established:
  - "Integration test pattern: setup_test_app() -> (Router, SqlitePool) with in-memory DB, migrations, and minimal config"
  - "seed_request() helper for inserting arbitrary test rows with sequential correlation IDs"
  - "parse_response() + get() helpers for oneshot HTTP testing returning (StatusCode, serde_json::Value)"

duration: 5min
completed: 2026-02-16
---

# Phase 11 Plan 02: Stats Integration Tests Summary

**14 integration tests for /v1/stats verifying aggregate queries, time ranges, filtering, grouping, empty results, and error handling against in-memory SQLite**

## Performance

- **Duration:** 5 min
- **Started:** 2026-02-16T17:31:37Z
- **Completed:** 2026-02-16T17:37:29Z
- **Tasks:** 2
- **Files modified:** 9

## Accomplishments
- 14 integration tests exercising all /v1/stats endpoint behaviors end-to-end
- Test helpers establishing reusable pattern for future integration tests (in-memory SQLite + tower::oneshot)
- Bug fix: COALESCE(AVG(), 0) changed to COALESCE(AVG(), 0.0) for SQLite f64 type compatibility on empty result sets
- Re-exported create_router from proxy module for integration test access
- Cargo fmt applied across codebase; clippy clean

## Task Commits

Each task was committed atomically:

1. **Task 1: Integration tests for /v1/stats endpoint** - `2fc8f20` (test)
2. **Task 2: Manual smoke test and fix any issues** - `ec34b31` (chore)

## Files Created/Modified
- `tests/stats.rs` - 14 integration tests with setup helpers, seed data, and comprehensive assertions
- `Cargo.toml` - Added tower (util feature) and http to dev-dependencies
- `src/proxy/mod.rs` - Added create_router to public re-exports
- `src/storage/stats.rs` - Fixed COALESCE fallback from integer 0 to float 0.0
- `src/main.rs` - Formatting only (cargo fmt)
- `src/proxy/stream.rs` - Formatting only (cargo fmt)
- `src/storage/logging.rs` - Formatting only (cargo fmt)
- `tests/env_expansion.rs` - Formatting only (cargo fmt)
- `tests/stream_options.rs` - Formatting only (cargo fmt)

## Decisions Made
- Used tower::ServiceExt::oneshot instead of TCP server bind for simpler, faster integration tests
- Created rfc3339z() helper to format timestamps with Z suffix instead of +00:00 (avoids URL encoding issues where + becomes space)
- Designed seed data with 4 records: 3 recent (mixed success/fail/streaming) and 1 old (8 days ago) to test time range boundaries

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed COALESCE(AVG(), 0) type mismatch on empty result sets**
- **Found during:** Task 1 (test_stats_empty_time_range)
- **Issue:** SQLite COALESCE(AVG(latency_ms), 0) returns INTEGER 0 when no rows match, but sqlx expects f64 for avg_latency_ms field
- **Fix:** Changed COALESCE fallback from 0 to 0.0 in both query_aggregate and query_grouped_by_model SQL
- **Files modified:** src/storage/stats.rs
- **Verification:** test_stats_empty_time_range passes (status 200, zeroed stats)
- **Committed in:** 2fc8f20 (Task 1 commit)

**2. [Rule 3 - Blocking] Exported create_router for integration test access**
- **Found during:** Task 1 (test compilation)
- **Issue:** create_router was pub in server.rs but not re-exported from proxy module; integration tests couldn't access it
- **Fix:** Added create_router to `pub use server::{}` in proxy/mod.rs
- **Files modified:** src/proxy/mod.rs
- **Verification:** tests/stats.rs compiles and all 14 tests pass
- **Committed in:** 2fc8f20 (Task 1 commit)

**3. [Rule 3 - Blocking] Added tower/http dev-dependencies for ServiceExt::oneshot**
- **Found during:** Task 1 (test compilation)
- **Issue:** tower crate lacked `util` feature needed for ServiceExt; http crate not in dev-deps
- **Fix:** Added `tower = { version = "0.4", features = ["util"] }` and `http = "1"` to [dev-dependencies]
- **Files modified:** Cargo.toml
- **Verification:** cargo build --test stats succeeds
- **Committed in:** 2fc8f20 (Task 1 commit)

**4. [Rule 1 - Bug] Fixed URL-encoded + sign in RFC 3339 timestamps**
- **Found during:** Task 1 (test_stats_explicit_time_range, test_stats_explicit_overrides_preset)
- **Issue:** chrono::to_rfc3339() produces `+00:00` suffix; the `+` is decoded as space in query params, causing 400 parse errors
- **Fix:** Created rfc3339z() helper using to_rfc3339_opts(SecondsFormat::Secs, true) to produce Z suffix
- **Files modified:** tests/stats.rs
- **Verification:** Both explicit time range tests pass (status 200)
- **Committed in:** 2fc8f20 (Task 1 commit)

---

**Total deviations:** 4 auto-fixed (2 bugs, 2 blocking)
**Impact on plan:** All auto-fixes necessary for correctness. No scope creep.

## Issues Encountered
None beyond the auto-fixed deviations above.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 11 complete: /v1/stats endpoint implemented and verified with 14 integration tests
- All 5 phase success criteria verified:
  - SC1: Aggregate summary with counts/costs/performance (test_stats_aggregate_default)
  - SC2: Per-model breakdown with zero-traffic models (test_stats_group_by_model)
  - SC3: ISO 8601 time range scoping (test_stats_explicit_time_range)
  - SC4: Range presets last_1h/24h/7d/30d (test_stats_aggregate_with_range_last_24h, last_30d)
  - SC5: Model/provider filtering with 404 (test_stats_filter_by_model, _provider, _404)
- Ready for Phase 12 (request log browsing) if milestone continues

---
*Phase: 11-aggregate-stats-and-filtering*
*Completed: 2026-02-16*
