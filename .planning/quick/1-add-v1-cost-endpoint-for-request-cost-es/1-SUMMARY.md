---
phase: quick
plan: 1
subsystem: api
tags: [cost-estimation, proxy, axum, routing]

# Dependency graph
requires:
  - phase: v1.5
    provides: proxy server, router, auth middleware
provides:
  - POST /v1/cost endpoint for pre-request cost estimation
  - 9 integration tests for cost endpoint
affects: [proxy, router, api]

# Tech tracking
tech-stack:
  added: []
  patterns: [read-only handler pattern (no upstream calls)]

key-files:
  created:
    - tests/cost.rs
  modified:
    - src/proxy/handlers.rs
    - src/proxy/server.rs

key-decisions:
  - "Input token estimation uses 4 chars/token heuristic (no tokenizer dependency)"
  - "Default output token estimate is 256 when max_tokens absent"

patterns-established:
  - "Read-only cost estimation handler: router.select() without upstream provider call"

requirements-completed: [COST-01]

# Metrics
duration: 3min
completed: 2026-03-03
---

# Quick Task 1: Add /v1/cost Endpoint Summary

**POST /v1/cost endpoint with 4-chars/token input estimation, max_tokens-based output estimation, and per-provider rate breakdown**

## Performance

- **Duration:** 3 min
- **Started:** 2026-03-03T00:34:46Z
- **Completed:** 2026-03-03T00:37:35Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- POST /v1/cost handler that accepts ChatCompletionRequest body and returns cost preview without calling upstream
- Provider selection via existing router (cheapest-first, policy-aware)
- Rate breakdown in response (input_rate, output_rate, base_fee per selected provider)
- 9 integration tests covering response shape, error cases, provider selection, token estimation, and auth

## Task Commits

Each task was committed atomically:

1. **Task 1: Add /v1/cost handler and route** - `5c6f1af` (feat)
2. **Task 2: Integration tests for /v1/cost** - `e79eb35` (test)

## Files Created/Modified
- `src/proxy/handlers.rs` - Added `cost_estimate` handler function
- `src/proxy/server.rs` - Registered POST /v1/cost route in auth-protected proxy_routes block
- `tests/cost.rs` - 9 integration tests for the /v1/cost endpoint

## Decisions Made
- Input token estimation uses sum of message content char lengths divided by 4 (minimum 1). No external tokenizer needed for cost preview use case.
- Default output token estimate is 256 when max_tokens is absent, providing a reasonable preview for typical requests.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- /v1/cost endpoint is live and auth-protected alongside /v1/chat/completions
- Future enhancement: token estimation could use tiktoken for more accurate counts

---
*Plan: quick-1*
*Completed: 2026-03-03*
