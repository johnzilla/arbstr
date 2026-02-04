---
phase: 02-request-logging
plan: 03
subsystem: api
tags: [axum, middleware, uuid, correlation-id, request-extensions]

# Dependency graph
requires:
  - phase: 01-foundation
    provides: TraceLayer with UUID in make_span_with
  - phase: 02-request-logging
    plan: 02
    provides: Database pool in AppState
provides:
  - RequestId newtype wrapping Uuid
  - inject_request_id middleware storing UUID in request extensions
  - make_span_with reading UUID from extensions (same UUID in span and extensions)
  - RequestId exported from proxy module for handler use
affects: [02-04, request-logging-handlers]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Middleware-based request ID injection via axum::middleware::from_fn"
    - "RequestId newtype pattern for type-safe extension extraction"
    - "Outermost middleware layer for ID generation (last .layer() call)"

key-files:
  created: []
  modified:
    - src/proxy/server.rs
    - src/proxy/mod.rs
    - src/proxy/types.rs
    - src/proxy/handlers.rs
    - src/config.rs
    - src/router/selector.rs

key-decisions:
  - "RequestId uses unwrap_or_else(Uuid::new_v4) fallback in make_span_with for robustness"
  - "Middleware is outermost layer (last .layer() call) so UUID is set before TraceLayer runs"

patterns-established:
  - "Extension-based request metadata: middleware sets, handlers extract via Extension<T>"
  - "Config::from_str renamed to Config::parse_str to avoid clippy should_implement_trait warning"

# Metrics
duration: 2min
completed: 2026-02-04
---

# Phase 2 Plan 3: Request ID Extensions Summary

**RequestId middleware injecting UUID into request extensions, readable by handlers via Extension<RequestId>**

## Performance

- **Duration:** 2 min
- **Started:** 2026-02-04T02:15:45Z
- **Completed:** 2026-02-04T02:18:00Z
- **Tasks:** 3
- **Files modified:** 6

## Accomplishments
- RequestId(Uuid) newtype defined with Clone + Debug derives
- inject_request_id middleware generates UUID and stores in request extensions
- make_span_with reads UUID from extensions instead of generating its own
- Same UUID present in both tracing span and request extensions
- RequestId exported from proxy module for handler use in 02-04

## Task Commits

Each task was committed atomically:

1. **Task 1: Add RequestId newtype and middleware to server.rs** - `a342c09` (feat)
2. **Task 2: Export RequestId from proxy module** - `f9c7f56` (feat)
3. **Task 3: Verify handlers can access RequestId** - no commit (verification-only, no code changes)

## Files Created/Modified
- `src/proxy/server.rs` - RequestId newtype, inject_request_id middleware, updated make_span_with
- `src/proxy/mod.rs` - Re-export RequestId
- `src/proxy/types.rs` - Added #[allow(dead_code)] on streaming types (clippy fix)
- `src/proxy/handlers.rs` - std::io::Error::other() clippy fix
- `src/config.rs` - Renamed from_str to parse_str (clippy fix)
- `src/router/selector.rs` - Collapsible if and manual retain clippy fixes

## Decisions Made
- RequestId uses unwrap_or_else(Uuid::new_v4) fallback in make_span_with for robustness if middleware is somehow bypassed
- Middleware is the outermost layer (last .layer() call in axum's reverse-order convention)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed pre-existing clippy warnings blocking -D warnings check**
- **Found during:** Task 1 (verification step)
- **Issue:** 7 pre-existing clippy errors: dead_code on streaming types, should_implement_trait on Config::from_str, io_other_error in handlers, collapsible_if and manual_retain in selector
- **Fix:** Added #[allow(dead_code)] on unused streaming types, renamed Config::from_str to Config::parse_str, used std::io::Error::other(), collapsed nested if, used .retain() instead of filter+collect
- **Files modified:** src/proxy/types.rs, src/config.rs, src/proxy/handlers.rs, src/router/selector.rs
- **Verification:** cargo clippy -- -D warnings passes clean
- **Committed in:** a342c09 (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Clippy fixes necessary for clean verification. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- RequestId available in handlers via Extension<RequestId> extractor
- Ready for 02-04 to add request logging to handlers using the correlation ID
- Database pool already in AppState from 02-02

---
*Phase: 02-request-logging*
*Completed: 2026-02-04*
