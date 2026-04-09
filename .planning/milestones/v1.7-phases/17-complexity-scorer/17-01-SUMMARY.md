---
phase: 17-complexity-scorer
plan: 01
subsystem: router
tags: [complexity, scoring, heuristics, routing]
dependency_graph:
  requires: [ComplexityWeightsConfig from Phase 16, Message/MessageContent types]
  provides: [score_complexity function, 5 weighted signals]
  affects: [Phase 18 tier mapping, Phase 19 routing integration]
tech_stack:
  added: [regex 1.x]
  patterns: [LazyLock regex compilation, sigmoid scoring curves, weighted average]
key_files:
  created:
    - src/router/complexity.rs
  modified:
    - Cargo.toml
    - src/config.rs
    - src/router/mod.rs
decisions:
  - Sigmoid curves for context_length (centered at 1000 tokens) and conversation_depth (centered at 6 messages) provide smooth gradation
  - File extension regex includes 16 common programming language extensions
  - Default-to-frontier (1.0) for empty, short, or all-zero-weight inputs ensures safe fallback
metrics:
  duration: 233s
  completed: "2026-04-08T21:42:21Z"
  tasks: 2
  files: 4
---

# Phase 17 Plan 01: Implement Complexity Scorer Summary

Heuristic complexity scorer with 5 weighted signals (context length, code blocks, multi-file indicators, reasoning keywords, conversation depth) using sigmoid curves and LazyLock-compiled regexes, returning 0.0-1.0 score for complexity-based routing.

## Changes Made

### Task 1: Implement complexity scorer with 5 weighted signals
**Commit:** f623fce

- Created `src/router/complexity.rs` with `score_complexity()` function
- Added `regex = "1"` dependency to Cargo.toml
- Added `extra_keywords: Vec<String>` field to `ComplexityWeightsConfig` in config.rs
- Implemented 5 signal functions each returning 0.0-1.0:
  - `signal_context_length`: Sigmoid centered at 1000 tokens (4000 chars)
  - `signal_code_blocks`: LazyLock regex counting fenced code blocks (0/1/2/3+ mapping)
  - `signal_multi_file`: LazyLock regex for file paths and extensions, unique match counting
  - `signal_reasoning_keywords`: 11 default keywords + configurable extra_keywords
  - `signal_conversation_depth`: Sigmoid centered at 6 messages
- Weighted average formula with clamp to [0.0, 1.0]
- Default-to-frontier (1.0) for empty messages or <10 chars total
- 13 unit tests covering all signals, weights, edge cases, multimodal content

### Task 2: Re-export scorer and verify full build
**Commit:** cdcbf8c

- Added `mod complexity` and `pub use complexity::score_complexity` to `src/router/mod.rs`
- Fixed pre-existing clippy `derivable_impls` lint on `Tier` enum (Rule 3 - blocking)
- Full test suite passes (153 tests), clippy clean

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed Tier enum clippy derivable_impls lint**
- **Found during:** Task 2
- **Issue:** Pre-existing `Tier` enum had manual `Default` impl that clippy flagged as derivable
- **Fix:** Replaced manual `impl Default` with `#[derive(Default)]` and `#[default]` attribute on `Standard` variant
- **Files modified:** src/config.rs
- **Commit:** cdcbf8c

## Verification Results

- `cargo test complexity` -- 13 complexity tests pass
- `cargo test` -- 153 total tests pass, no regressions
- `cargo clippy -- -D warnings` -- clean, no warnings
- `score_complexity(&[], &default)` returns 1.0 (frontier default)
- "Hello, how are you today?" scores below 0.4
- Complex multi-turn with code + keywords scores above 0.7

## Self-Check: PASSED

- src/router/complexity.rs: FOUND
- src/router/mod.rs: FOUND
- src/config.rs: FOUND
- Commit f623fce: FOUND
- Commit cdcbf8c: FOUND
