---
phase: 01-foundation
plan: 02
subsystem: infra
tags: [tracing, uuid, correlation-id, tower-http, axum]

# Dependency graph
requires:
  - phase: none
    provides: existing proxy server with TraceLayer
provides:
  - per-request UUID v4 correlation ID on tracing span
  - make_span_with closure pattern for TraceLayer configuration
affects: [02-logging, 03-headers]

# Tech tracking
tech-stack:
  added: []
  patterns: [TraceLayer make_span_with for per-request span customization]

key-files:
  created: []
  modified: [src/proxy/server.rs]

key-decisions:
  - "UUID v4 generated internally by arbstr, not read from client headers"
  - "info_span used (not debug_span) so correlation ID visible at default log level"
  - "request_id not added as axum extension yet (deferred to Phase 3)"

patterns-established:
  - "Per-request tracing span: all downstream log events inherit request_id field"
  - "TraceLayer make_span_with closure pattern for middleware customization"

# Metrics
duration: 3min
completed: 2026-02-02
---

# Phase 1 Plan 2: Request Correlation ID Summary

**TraceLayer configured with make_span_with generating UUID v4 per-request correlation ID on info_span**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-03T03:40:55Z
- **Completed:** 2026-02-03T03:44:00Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- TraceLayer now generates a unique UUID v4 `request_id` per HTTP request
- Correlation ID appears on the tracing span wrapping the entire request lifecycle
- All downstream log events within a request automatically inherit the `request_id` field
- Smoke test confirmed two different requests produce different UUIDs in structured log output

## Task Commits

Each task was committed atomically:

1. **Task 1: Configure TraceLayer with per-request UUID correlation ID** - `5700d7c` (feat)
2. **Task 2: Verify correlation ID appears in log output** - no commit (verification-only task)

## Files Created/Modified
- `src/proxy/server.rs` - Added `use uuid::Uuid` import; replaced bare `TraceLayer::new_for_http()` with configured `make_span_with` closure that generates UUID v4 and attaches `request_id`, `method`, and `uri` fields to an `info_span!("request", ...)`

## Decisions Made
- Used `axum::http::Request<axum::body::Body>` for the closure type parameter, consistent with existing codebase pattern (`axum::http::StatusCode` in error.rs)
- UUID v4 is generated server-side only -- not read from `X-Request-ID` client headers (arbstr controls the ID)
- Used `info_span!` (not `debug_span!`) so correlation IDs appear at default log levels
- Did not add request_id as an axum extension -- that is a Phase 3 concern (response headers)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
- Pre-existing clippy warnings in other files (`types.rs`, `config.rs`, `handlers.rs`, `selector.rs`) cause `cargo clippy -- -D warnings` to fail project-wide, but none are in `server.rs` and none were introduced by this change
- Discovered uncommitted changes from plan 01-01 in the working tree (`selector.rs`, `router/mod.rs`) -- these were left untouched and only `server.rs` was committed

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- Correlation ID is on the tracing span and ready for Phase 2 (SQLite request logging) to extract and store
- Phase 3 (response headers) can read the span's `request_id` to return it to clients
- No blockers

---
*Phase: 01-foundation*
*Completed: 2026-02-02*
