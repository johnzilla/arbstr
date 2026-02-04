# Project State

## Project Reference

See: .planning/PROJECT.md (updated 2026-02-02)

**Core value:** Smart model selection that minimizes sats spent per request without sacrificing quality
**Current focus:** Phase 4 - Retry and Fallback (Plan 02 of 03 complete)

## Current Position

Phase: 4 of 4 (Retry and Fallback)
Plan: 2 of 3 in current phase
Status: In progress
Last activity: 2026-02-04 -- Completed 04-02-PLAN.md (retry module with retry-with-fallback logic)

Progress: [█████████░] 90%

## Performance Metrics

**Velocity:**
- Total plans completed: 9
- Average duration: 2 min
- Total execution time: 0.3 hours

**By Phase:**

| Phase | Plans | Total | Avg/Plan |
|-------|-------|-------|----------|
| 1. Foundation | 2/2 | 6 min | 3 min |
| 2. Request Logging | 4/4 | 7 min | 2 min |
| 3. Response Metadata | 1/1 | 2 min | 2 min |
| 4. Retry and Fallback | 2/3 | 4 min | 2 min |

**Recent Trend:**
- Last 5 plans: 02-03 (2 min), 02-04 (2 min), 03-01 (2 min), 04-01 (2 min), 04-02 (2 min)
- Trend: Consistent, stable at ~2 min

*Updated after each plan completion*

## Accumulated Context

### Decisions

Decisions are logged in PROJECT.md Key Decisions table.
Recent decisions affecting current work:

- Roadmap: Fix cost calculation before logging (broken formula pollutes historical data)
- Roadmap: Streaming observability deferred to v2 (OBSRV-12 out of scope)
- Roadmap: 4 phases derived from 3 requirement categories with observability split into logging + headers
- 01-01: Routing heuristic uses output_rate + base_fee (not full formula) since token counts unknown at selection time
- 01-01: actual_cost_sats returns f64 for sub-satoshi precision
- 01-02: UUID v4 generated internally by arbstr, not read from client headers
- 01-02: info_span used (not debug_span) so correlation ID visible at default log level
- 02-01: MigrateError converts to sqlx::Error via ? operator, no Box<dyn Error> needed
- 02-01: Storage module declared in lib.rs during 02-01 (not 02-02) to verify compilation
- 02-03: RequestId uses unwrap_or_else fallback in make_span_with for robustness
- 02-03: Config::from_str renamed to Config::parse_str to satisfy clippy should_implement_trait
- 02-04: Streaming requests logged with None tokens/cost (stream not consumed at log time)
- 02-04: Core logic extracted into execute_request returning Result<RequestOutcome, RequestError> for unified logging
- 03-01: Error path returns Ok(error_response) with arbstr headers instead of Err(Error)
- 03-01: Streaming responses omit cost and latency headers, include streaming flag
- 03-01: Cost formatted with 2 decimal places (e.g. 0.10 not 0.1)
- 04-01: select_candidates returns Vec<SelectedProvider> sorted by routing cost, select delegates to it
- 04-01: Removed dead select_cheapest/select_first methods; default_strategy retained with allow(dead_code)
- 04-02: BACKOFF_DURATIONS uses [Duration; 3] = [1s, 2s, 4s] matching locked decision
- 04-02: retry_with_fallback is generic over T/E with HasStatusCode trait, not coupled to handler types
- 04-02: Attempts tracked via Arc<Mutex<Vec<AttemptRecord>>> parameter for timeout-safe tracking
- 04-02: Non-retryable errors (4xx) skip fallback entirely

### Pending Todos

None yet.

### Blockers/Concerns

- Research flag: Cashu token double-spend semantics during retry need verification before Phase 4 planning
- Research flag: Routstr SSE streaming format (usage field in final chunk) affects future v2 streaming work

## Session Continuity

Last session: 2026-02-04
Stopped at: Completed 04-02-PLAN.md. Ready for 04-03 (handler integration with retry/fallback).
Resume file: None
