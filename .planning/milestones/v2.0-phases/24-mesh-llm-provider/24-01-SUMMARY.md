---
phase: 24-mesh-llm-provider
plan: 01
subsystem: proxy/discovery
tags: [discovery, mesh-llm, config, auto-discover]
dependency_graph:
  requires: []
  provides: [auto_discover_config, discover_models_function, mesh_llm_template]
  affects: [src/config.rs, src/proxy/server.rs, config.example.toml]
tech_stack:
  added: []
  patterns: [startup-discovery, graceful-degradation]
key_files:
  created:
    - src/proxy/discovery.rs
    - tests/discovery.rs
  modified:
    - src/config.rs
    - src/proxy/mod.rs
    - src/proxy/server.rs
    - config.example.toml
    - src/router/selector.rs
    - src/main.rs
    - tests/common/mod.rs
    - tests/cost.rs
    - tests/escalation.rs
    - tests/health.rs
    - tests/circuit_integration.rs
    - /home/john/vault/projects/github.com/arbstr-node/config.toml
decisions:
  - "auto_discover defaults to false via serde(default) for backward compatibility"
  - "5-second per-request timeout for discovery to prevent slow endpoints blocking startup"
  - "Discovery replaces static models list (not merge) per D-03"
  - "Server startup reordered: http_client created before ProviderRouter to enable discovery"
metrics:
  duration: 6m
  completed: 2026-04-10
  tasks_completed: 2
  tasks_total: 2
  tests_added: 6
  files_changed: 14
---

# Phase 24 Plan 01: Auto-discover Model Discovery Summary

Startup model discovery for providers with OpenAI-compatible /v1/models endpoints, enabling mesh-llm local inference nodes as zero-config providers.

## What Was Done

### Task 1: Add auto_discover config field, discovery function, and server integration
- Added `auto_discover: bool` field with `#[serde(default)]` to `ProviderConfig` and `RawProviderConfig`
- Created `src/proxy/discovery.rs` with `discover_models()` function: iterates providers with `auto_discover=true`, GETs `/v1/models`, replaces static models list with discovered IDs
- 5-second per-request timeout via `reqwest::Client::get().timeout(Duration::from_secs(5))`
- URL normalization via `trim_end_matches('/')`
- Graceful degradation: unreachable providers keep static models, warns if empty
- Registered `pub mod discovery` in `src/proxy/mod.rs`
- Reordered `run_server()` in `server.rs`: `http_client` creation before `ProviderRouter::new()` to support discovery
- Changed `run_server` signature from `config: Config` to `mut config: Config`
- Updated `config.example.toml` with `# auto_discover = false` on existing providers and commented mesh-llm example block
- Created `tests/discovery.rs` with 6 integration tests using wiremock
- Updated all `ProviderConfig` struct literals across source and test files (13 files) to include `auto_discover: false`

**Commits:** `5011a33` (RED - failing tests), `cf46883` (GREEN - implementation)

### Task 2: Update arbstr-node config template with mesh-llm provider example
- Replaced static `models = ["Qwen3-32B"]` with `auto_discover = true` in mesh-local block
- Changed rates from `input_rate = 5` / `output_rate = 15` to zero-cost (`input_rate = 0` / `output_rate = 0`)
- Added `# auto_discover = false` to routstr provider example
- Added descriptive comment about automatic model discovery
- Verified `extra_hosts: host.docker.internal:host-gateway` present in docker-compose.yml (MESH-03 satisfied)

**Commit:** `35b3335` (arbstr-node repo)

## Deviations from Plan

None -- plan executed exactly as written.

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| `auto_discover` defaults to `false` via `serde(default)` | Backward compatibility: existing configs without the field work identically |
| 5-second per-request timeout | Prevents slow/hung local endpoints from blocking startup (T-24-01 mitigation) |
| Replace (not merge) static models on discovery | Consistent behavior: discovered state is authoritative (D-03) |
| Reorder http_client before ProviderRouter in server.rs | Discovery needs the HTTP client; router needs discovered models |

## Verification Results

- `cargo test --test discovery` -- 6/6 pass (success, unreachable, empty, skip, backward_compat, replace)
- `cargo test` -- all 250 tests pass (0 failures, 0 regressions)
- `cargo clippy -- -D warnings` -- clean
- arbstr-node config.toml contains auto_discover=true, host.docker.internal:9337, tier=local, zero-cost
- arbstr-node docker-compose.yml contains host.docker.internal:host-gateway (pre-existing)

## Self-Check: PASSED

All files exist, all commits verified, all acceptance criteria met.
