---
phase: 20-routing-observability
plan: 01
title: "Complexity Observability"
one_liner: "Surface complexity score and tier via response headers, SSE metadata, DB columns, and INFO logging"
completed: "2026-04-08"
duration_minutes: 8
tasks_completed: 2
tasks_total: 2
artifacts_created:
  - migrations/20260409000000_add_complexity_columns.sql
artifacts_modified:
  - src/storage/logging.rs
  - src/storage/writer.rs
  - src/proxy/handlers.rs
key_decisions:
  - "Used #[allow(clippy::too_many_arguments)] on log_error_to_db rather than refactoring to a struct (minimal churn)"
  - "Complexity headers sent on both streaming and non-streaming paths at header-send time (score/tier known before provider call)"
  - "Tier stored as String in DB (from Tier::Display) for flexibility; enum has 3 fixed variants so no injection risk"
---

# Phase 20 Plan 01: Complexity Observability Summary

Surface complexity score and tier via response headers, SSE trailing metadata, DB columns, and INFO logging across all request paths.

## Commits

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 | Migration, RequestLog extension, INSERT/UPDATE SQL | 6cdaf35 | migrations/20260409000000_add_complexity_columns.sql, src/storage/logging.rs, src/storage/writer.rs |
| 2 | Response headers, SSE metadata, INFO logging, handler DB plumbing | abfac3d | src/proxy/handlers.rs |

## What Changed

### Task 1: Storage Layer
- Created migration adding `complexity_score REAL` and `tier TEXT` nullable columns to `requests` table
- Extended `RequestLog` struct with `complexity_score: Option<f64>` and `tier: Option<String>`
- Updated INSERT SQL from 14 to 16 columns with corresponding bind params
- Extended `update_stream_completion` and `spawn_stream_completion_update` with two new parameters
- Updated `WriteCommand::UpdateStreamCompletion` variant and writer_loop match arm
- All test helpers updated with new fields

### Task 2: Handler Layer
- Added `ARBSTR_COMPLEXITY_SCORE_HEADER` and `ARBSTR_TIER_HEADER` constants
- Non-streaming success responses now include `x-arbstr-complexity-score` (3dp) and `x-arbstr-tier` headers
- Streaming success responses include both headers at header-send time
- `build_trailing_sse_event` extended: JSON now includes `complexity_score` and `tier` fields
- INFO log emitted per routed request: `complexity_score`, `tier`, `provider` structured fields
- `log_success_to_db` and `log_error_to_db` accept and pass complexity_score/tier
- All call sites updated (resolve_candidates errors, vault errors, streaming/non-streaming paths)
- Complexity data threaded through `send_to_provider` -> `handle_streaming_response` -> spawned task

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo test` -- full suite green (all 157+ unit tests and integration tests pass)
- `cargo clippy -- -D warnings` -- clean

## Self-Check: PASSED
