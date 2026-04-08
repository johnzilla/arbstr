# Phase 16: Provider Tier Foundation - Context

**Gathered:** 2026-04-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Add a `Tier` enum (`local`/`standard`/`frontier`) to provider configuration with backward-compatible defaults. Propagate tier through the routing pipeline types. Add a `[routing]` config skeleton with complexity threshold fields. No routing logic changes -- tier-aware routing happens in Phase 18.

</domain>

<decisions>
## Implementation Decisions

### Tier type design
- **D-01:** `Tier` enum with three variants: `Local`, `Standard`, `Frontier`
- **D-02:** Ordering is `Local < Standard < Frontier` -- derive `Ord`/`PartialOrd` so router can filter with `provider.tier <= max_tier`
- **D-03:** Serialize/deserialize as lowercase strings: `"local"`, `"standard"`, `"frontier"` in TOML, DB, logs, and API responses
- **D-04:** Default value is `Standard` (via `#[serde(default)]` with default function) for backward compatibility

### Config surface
- **D-05:** Add optional `tier` field to `[[providers]]` in config.toml
- **D-06:** Add `[routing]` config section now with `complexity_threshold_low` (default 0.4) and `complexity_threshold_high` (default 0.7) fields -- parsed but unused until Phase 18
- **D-07:** Add `[routing.complexity_weights]` sub-section with signal weight fields (all default 1.0) -- parsed but unused until Phase 17

### Propagation scope
- **D-08:** Add `tier: Tier` field to `SelectedProvider` struct and update `From<&ProviderConfig>` impl
- **D-09:** Expose tier in `/providers` endpoint response JSON
- **D-10:** Show tier in `/health` per-provider status

### Claude's Discretion
- Exact placement of `Tier` type (new file vs inline in config.rs vs router module)
- Whether to use `#[serde(rename_all = "lowercase")]` or custom Serialize/Deserialize impl
- `RoutingConfig` struct field naming conventions

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Provider config
- `src/config.rs` -- `ProviderConfig` struct (line ~148), `Config` root struct, serde default patterns
- `src/router/selector.rs` -- `SelectedProvider` struct (line ~10), `From<&ProviderConfig>` impl (line ~19), `select_candidates` method (line ~87)

### Endpoint responses
- `src/proxy/handlers.rs` -- `/providers` endpoint handler, `/health` endpoint handler
- `src/proxy/circuit_breaker.rs` -- per-provider state reported in health

### Config example
- `config.example.toml` -- must be updated with `tier` field and `[routing]` section

### Research
- `.planning/research/ARCHITECTURE.md` -- integration architecture, suggested `Tier` enum design
- `.planning/research/PITFALLS.md` -- backward compatibility requirements, serde default patterns

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `#[serde(default)]` pattern used extensively in config.rs (ServerConfig, ProviderConfig fields)
- `default_*()` function pattern for serde defaults (e.g., `default_listen`, `default_strategy`)
- Existing `From<&ProviderConfig> for SelectedProvider` conversion impl to extend

### Established Patterns
- Config structs are `Debug + Clone + Deserialize` (not Serialize -- one-directional)
- Optional config sections use `Option<T>` on the parent struct (e.g., `vault: Option<VaultConfig>`)
- New config sections with all-default fields can use `#[serde(default)]` on the parent field

### Integration Points
- `ProviderConfig` is consumed by `Router::new()` in selector.rs
- `SelectedProvider` flows through handlers.rs retry loop, circuit breaker checks, and DB logging
- `/providers` handler in handlers.rs serializes provider info for the API response
- `/health` handler includes per-provider circuit breaker state

</code_context>

<specifics>
## Specific Ideas

No specific requirements -- open to standard approaches. The user's original spec shows the exact TOML format:

```toml
[[providers]]
name = "local-mesh"
url = "http://localhost:8089/v1"
models = ["mistral", "nemotron", "mixtral"]
input_rate = 0
output_rate = 0
tier = "local"
```

</specifics>

<deferred>
## Deferred Ideas

None -- discussion stayed within phase scope.

</deferred>

---

*Phase: 16-provider-tier-foundation*
*Context gathered: 2026-04-08*
