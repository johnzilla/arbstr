---
phase: 16-provider-tier-foundation
plan: 01
subsystem: config
tags: [tier, routing, config, complexity]
dependency_graph:
  requires: []
  provides: [Tier enum, RoutingConfig, ComplexityWeightsConfig, tier field on ProviderConfig]
  affects: [src/config.rs, config.example.toml, src/main.rs, src/router/selector.rs]
tech_stack:
  added: []
  patterns: [serde rename_all lowercase, Ord-derived enum ordering, serde default functions]
key_files:
  created: []
  modified:
    - src/config.rs
    - config.example.toml
    - src/main.rs
    - src/router/selector.rs
    - tests/common/mod.rs
    - tests/cost.rs
    - tests/health.rs
    - tests/circuit_integration.rs
decisions:
  - Tier enum placed before ProviderConfig in config.rs (after KeySource enum)
  - Tier uses derive(Ord) for natural Local < Standard < Frontier ordering
  - Routing config uses serde(default) at both field and struct level for full backward compatibility
metrics:
  duration: 498s
  completed: 2026-04-08T18:02:00Z
  tasks_completed: 2
  tasks_total: 2
  test_count: 140 lib + 65 integration = 205 total
---

# Phase 16 Plan 01: Tier Enum and Config Foundation Summary

Tier enum with Ord-derived Local < Standard < Frontier ordering, RoutingConfig with complexity thresholds (0.4/0.7), and ComplexityWeightsConfig with 5 signal weight fields defaulting to 1.0 -- all backward compatible via serde defaults.

## What Was Done

### Task 1: Add Tier enum and RoutingConfig structs (TDD)
- Added `Tier` enum with `Local`, `Standard`, `Frontier` variants
- Derives: `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize`
- `#[serde(rename_all = "lowercase")]` for JSON/TOML serialization
- `Default` impl returns `Tier::Standard`
- `Display` impl for logging
- Added `RoutingConfig` with `complexity_threshold_low` (0.4) and `complexity_threshold_high` (0.7)
- Added `ComplexityWeightsConfig` with 5 signal weight fields all defaulting to 1.0
- 7 new unit tests covering ordering, serde, defaults
- Commit: `c0cb514`

### Task 2: Wire tier and routing into config structs
- Added `tier: Tier` field to `ProviderConfig` and `RawProviderConfig` with `#[serde(default)]`
- Added `routing: RoutingConfig` field to `Config` and `RawConfig` with `#[serde(default)]`
- Updated `from_raw_with_lookup` to propagate both fields
- Updated all `ProviderConfig` and `Config` literals across 8 files (src + tests)
- Updated `config.example.toml` with `tier` field and commented `[routing]` section
- 4 new backward compatibility tests confirming existing configs parse unchanged
- Commit: `53915c6`

## Deviations from Plan

None - plan executed exactly as written.

## Verification Results

- `cargo test`: 205 tests pass (140 lib + 65 integration), 0 failures
- `pub enum Tier` exists at line 156 of src/config.rs
- `pub tier: Tier` on ProviderConfig at line 271
- `pub routing: RoutingConfig` on Config at line 20
- config.example.toml contains `tier = "standard"` and `# complexity_threshold_low = 0.4`
- Backward compatibility confirmed: configs without `tier` or `[routing]` parse with correct defaults

## Self-Check: PASSED
