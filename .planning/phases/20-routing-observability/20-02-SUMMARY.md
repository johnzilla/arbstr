---
phase: 20-routing-observability
plan: 02
title: "Tier Stats Grouping"
one_liner: "GET /v1/stats?group_by=tier returns per-tier cost and performance breakdown with COALESCE NULL as unknown"
completed: "2026-04-08"
duration_minutes: 3
tasks_completed: 1
tasks_total: 1
artifacts_modified:
  - src/storage/stats.rs
  - src/proxy/stats.rs
  - tests/stats.rs
key_decisions:
  - "Known tiers (local/standard/frontier) always shown in response with zeroed stats when no traffic"
  - "NULL tier coalesced to 'unknown' in SQL GROUP BY"
  - "tier_row_to_json mirrors model_row_to_json shape for consistent API surface"
---

# Phase 20 Plan 02: Tier Stats Grouping Summary

GET /v1/stats?group_by=tier returns per-tier cost and performance breakdown with COALESCE NULL as unknown.

## Commits

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 (RED) | Failing tests for group_by=tier | 2824f0b | tests/stats.rs |
| 1 (GREEN) | TierRow query, handler, validation | 86ec8cd | src/storage/stats.rs, src/proxy/stats.rs |

## What Changed

### Task 1: TierRow query, stats handler group_by=tier, and integration tests

**Storage layer (src/storage/stats.rs):**
- Added `TierRow` struct (mirrors `ModelRow` with `tier: String` instead of `model: String`)
- Added `query_grouped_by_tier()` function with `COALESCE(tier, 'unknown')` grouping
- Supports optional model and provider filters

**Handler layer (src/proxy/stats.rs):**
- Expanded `group_by` validation to accept `"model"` and `"tier"` (error message updated)
- Added `tiers` field to `StatsResponse` with `skip_serializing_if = "Option::is_none"`
- When `group_by=tier`: queries tier data, builds JSON map with known tiers (local/standard/frontier) plus any SQL-only tiers (e.g., "unknown")
- Added `tier_row_to_json()` helper matching `model_row_to_json()` shape

**Integration tests (tests/stats.rs):**
- `test_stats_group_by_tier_empty` -- no data returns tiers object with zeroed entries
- `test_stats_group_by_tier_with_data` -- per-tier breakdown sums correctly (local/standard/frontier)
- `test_stats_group_by_tier_null_tier` -- NULL tier appears under "unknown" key
- `test_stats_group_by_invalid_400` -- invalid group_by returns 400 mentioning both "model" and "tier"

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo test` -- full suite green (all existing + 4 new tier stats tests)
- `cargo clippy -- -D warnings` -- clean

## Self-Check: PASSED
