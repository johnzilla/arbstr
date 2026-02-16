---
phase: 08-stream-request-foundation
plan: 01
subsystem: api
tags: [streaming, openai, sse, sqlite, stream_options]

# Dependency graph
requires:
  - phase: 04-request-logging
    provides: SQLite request log table and spawn_log_write pattern
provides:
  - StreamOptions struct and stream_options field on ChatCompletionRequest
  - ensure_stream_options merge function for streaming request injection
  - update_usage and spawn_usage_update for post-stream database writes
affects: [09-sse-usage-extraction, 10-stream-usage-reconciliation]

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "Merge-not-overwrite for client-provided stream_options"
    - "Clone-and-mutate pattern for injecting fields at send time"
    - "Fire-and-forget UPDATE pattern mirroring spawn_log_write"

key-files:
  created:
    - tests/stream_options.rs
  modified:
    - src/proxy/types.rs
    - src/proxy/mod.rs
    - src/proxy/handlers.rs
    - src/storage/logging.rs
    - src/storage/mod.rs

key-decisions:
  - "Merge semantics for stream_options: only set include_usage when is_none, preserve client-provided false"
  - "Inject at send time via clone-and-mutate, not at parse time, keeping original request immutable"
  - "update_usage writes tokens and cost only; latency stays as TTFB from INSERT"

patterns-established:
  - "ensure_stream_options: merge-not-overwrite for OpenAI-compatible field injection"
  - "spawn_usage_update: fire-and-forget UPDATE mirroring spawn_log_write INSERT pattern"

# Metrics
duration: 3min
completed: 2026-02-16
---

# Phase 8 Plan 1: Stream Request Foundation Summary

**StreamOptions injection for streaming requests and post-stream UPDATE path for usage reconciliation**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-16T13:08:24Z
- **Completed:** 2026-02-16T13:11:23Z
- **Tasks:** 2
- **Files modified:** 5

## Accomplishments
- StreamOptions struct with include_usage field and proper serde skip_serializing_if
- ensure_stream_options merge function that preserves client-provided values
- send_to_provider injects stream_options only for streaming requests via clone-and-mutate
- update_usage and spawn_usage_update for writing tokens/cost after stream completion
- 13 new tests (6 unit in types.rs, 4 integration in stream_options.rs, 3 database in logging.rs)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add StreamOptions type and inject into streaming requests at send time** - `4e44628` (feat)
2. **Task 2: Add post-stream database UPDATE for token counts and cost** - `0b37923` (feat)

## Files Created/Modified
- `src/proxy/types.rs` - StreamOptions struct, stream_options field, ensure_stream_options function, 6 unit tests
- `src/proxy/mod.rs` - Made types module public, exported StreamOptions and ensure_stream_options
- `src/proxy/handlers.rs` - Clone-and-mutate injection in send_to_provider for streaming requests
- `src/storage/logging.rs` - update_usage async function, spawn_usage_update fire-and-forget wrapper, 3 tests
- `src/storage/mod.rs` - Exported update_usage and spawn_usage_update
- `tests/stream_options.rs` - 4 integration tests for injection, merge, and roundtrip behavior

## Decisions Made
- Merge semantics for stream_options: only set include_usage to true when is_none, preserve client-provided false value
- Inject stream_options at send time (in send_to_provider) via clone, not at request parse time, keeping the original request immutable throughout the handler chain
- update_usage writes only token counts and cost_sats; latency stays as TTFB recorded at INSERT time (Phase 10 handles full-stream latency if needed)

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- stream_options injection is active: all streaming requests will now ask providers for usage data in the final SSE chunk
- update_usage is available for Phase 10 (stream usage reconciliation) to write extracted tokens back to the request log
- Phase 9 (SSE usage extraction) can proceed independently to build the stream-wrapping logic that captures usage from SSE chunks

## Self-Check: PASSED

All 6 source files verified present. Both task commits (4e44628, 0b37923) verified in git log.

---
*Phase: 08-stream-request-foundation*
*Completed: 2026-02-16*
