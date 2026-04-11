# Phase 24: mesh-llm Provider - Context

**Gathered:** 2026-04-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Make mesh-llm nodes on localhost usable as zero-cost local-tier providers with automatic model discovery. Add `auto_discover` config field, startup model polling, and a commented-out mesh-llm example in arbstr-node config.

</domain>

<decisions>
## Implementation Decisions

### Model Discovery Mechanism
- **D-01:** Startup poll only. On startup, for each provider with `auto_discover = true`, GET `{provider.url}/v1/models` and populate the provider's model list from the response. No periodic refresh, no background tasks.
- **D-02:** If the provider is unreachable at startup, log a warning and continue. Provider stays registered with zero models (won't match any requests). Non-blocking startup — arbstr starts fine even if mesh-llm isn't running.
- **D-03:** Discovery results replace any statically configured `models` list. If `auto_discover = true` and `models = ["fallback-model"]`, discovered models take precedence; static list is only used if discovery fails.

### Provider Type Flag
- **D-04:** Generic `auto_discover: bool` field on `ProviderConfig`, default `false`. Any provider with an OpenAI-compatible `/v1/models` endpoint can opt in — not mesh-llm-specific. No `type` enum needed.
- **D-05:** Backward compatible — existing configs without `auto_discover` continue to work identically.

### Model Name Handling
- **D-06:** Use exact model names as returned by `/v1/models` (e.g., `Qwen3-8B-Q4_K_M`). No normalization, no base-name aliases, no prefix matching. Clients must request the exact model name.
- **D-07:** arbstr's own `/v1/models` endpoint exposes all discovered model names, so clients can enumerate available models to find exact names.

### Compose Config Template
- **D-08:** Ship mesh-llm provider entry as a commented-out example in arbstr-node's `config.toml`. Users uncomment when they have mesh-llm running. Avoids startup warnings when mesh-llm isn't installed.
- **D-09:** Example uses `http://host.docker.internal:9337/v1` (Docker networking), `tier = "local"`, `auto_discover = true`, zero-cost rates.

### Claude's Discretion
- How to structure the startup discovery code (where in server initialization, error handling details)
- Whether to add a timeout for the /v1/models request (reasonable default like 5s)
- Whether /v1/models response needs pagination handling (unlikely for mesh-llm)
- Exact log message formatting for discovery success/failure
- Whether to update config.example.toml in the arbstr repo as well

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Provider Configuration
- `src/config.rs` — ProviderConfig struct (line ~261), needs `auto_discover` field added
- `src/router/selector.rs` — Provider selection logic, uses `provider.models` for matching
- `config.example.toml` — Example config file in arbstr repo

### Server Initialization
- `src/proxy/server.rs` — Server startup, AppState construction — discovery runs here before serving
- `src/main.rs` — CLI entry point, config loading

### Model Listing
- `src/proxy/handlers.rs` — /v1/models handler, needs to include discovered models

### Docker Compose
- `/home/john/vault/projects/github.com/arbstr-node/config.toml` — arbstr-node config, add commented mesh-llm example
- `/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml` — already has extra_hosts for host.docker.internal

### Research
- `.planning/research/ARCHITECTURE.md` — mesh-llm integration analysis, anti-patterns (don't containerize mesh-llm)
- `.planning/research/PITFALLS.md` — Pitfall 5 (model name mismatch), Pitfall 6 (node disappears mid-inference), Pitfall 14 (Docker NAT)
- `.planning/research/STACK.md` — mesh-llm provider support section (no new crates)
- `.planning/research/FEATURES.md` — mesh-llm feature analysis and effort estimates

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `ProviderConfig` struct in `config.rs` — add `auto_discover: bool` field with `#[serde(default)]`
- `reqwest::Client` in `AppState` — reuse for /v1/models HTTP call during discovery
- Circuit breaker in `circuit_breaker.rs` — already handles mesh-llm provider failures, no changes needed
- `/v1/models` handler in `handlers.rs` — already aggregates models from all providers

### Established Patterns
- `#[serde(default)]` for backward-compatible config fields (used for `tier`, `base_fee`, etc.)
- Startup initialization in `server.rs` — discovery fits after config load, before server bind
- `tracing::warn!` / `tracing::info!` for startup diagnostics
- `reqwest` for HTTP calls to providers (same client used for inference forwarding)

### Integration Points
- `ProviderConfig.models` — currently `Vec<String>`, populated from config. Discovery would mutate this before `ProviderRouter` is constructed
- `ProviderRouter::new()` — receives `Vec<ProviderConfig>` — discovery runs before this, modifying the configs in place
- arbstr-node `config.toml` — add commented mesh-llm provider block

</code_context>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 24-mesh-llm-provider*
*Context gathered: 2026-04-10*
