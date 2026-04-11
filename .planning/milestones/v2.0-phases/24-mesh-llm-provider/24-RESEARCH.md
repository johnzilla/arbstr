# Phase 24: mesh-llm Provider - Research

**Researched:** 2026-04-10
**Domain:** Provider auto-discovery, OpenAI-compatible model listing, Docker host networking
**Confidence:** HIGH

## Summary

Phase 24 adds automatic model discovery for providers with an OpenAI-compatible `/v1/models` endpoint, with mesh-llm as the primary use case. The implementation is straightforward: add an `auto_discover: bool` field to `ProviderConfig`, poll `/v1/models` during startup for providers that opt in, and mutate the provider's `models` list before constructing `ProviderRouter`. No new crates are needed -- the existing `reqwest::Client` handles the HTTP call.

The main integration points are well-defined: `ProviderConfig` in `config.rs` gets a new field, `run_server()` in `server.rs` gets a discovery step between config load and router construction, and arbstr-node's `config.toml` gets a commented-out mesh-llm example with `auto_discover = true`. Docker networking is already handled -- `extra_hosts: host.docker.internal:host-gateway` is present in the compose file.

**Primary recommendation:** Implement as a single async function `discover_models()` that takes `&mut Vec<ProviderConfig>` and `&Client`, iterating providers with `auto_discover = true` and replacing their `models` vec with discovered names. Call it in `run_server()` after HTTP client creation, before `ProviderRouter::new()`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- **D-01:** Startup poll only. On startup, for each provider with `auto_discover = true`, GET `{provider.url}/v1/models` and populate the provider's model list from the response. No periodic refresh, no background tasks.
- **D-02:** If the provider is unreachable at startup, log a warning and continue. Provider stays registered with zero models (won't match any requests). Non-blocking startup -- arbstr starts fine even if mesh-llm isn't running.
- **D-03:** Discovery results replace any statically configured `models` list. If `auto_discover = true` and `models = ["fallback-model"]`, discovered models take precedence; static list is only used if discovery fails.
- **D-04:** Generic `auto_discover: bool` field on `ProviderConfig`, default `false`. Any provider with an OpenAI-compatible `/v1/models` endpoint can opt in -- not mesh-llm-specific. No `type` enum needed.
- **D-05:** Backward compatible -- existing configs without `auto_discover` continue to work identically.
- **D-06:** Use exact model names as returned by `/v1/models` (e.g., `Qwen3-8B-Q4_K_M`). No normalization, no base-name aliases, no prefix matching. Clients must request the exact model name.
- **D-07:** arbstr's own `/v1/models` endpoint exposes all discovered model names, so clients can enumerate available models to find exact names.
- **D-08:** Ship mesh-llm provider entry as a commented-out example in arbstr-node's `config.toml`. Users uncomment when they have mesh-llm running. Avoids startup warnings when mesh-llm isn't installed.
- **D-09:** Example uses `http://host.docker.internal:9337/v1` (Docker networking), `tier = "local"`, `auto_discover = true`, zero-cost rates.

### Claude's Discretion
- How to structure the startup discovery code (where in server initialization, error handling details)
- Whether to add a timeout for the /v1/models request (reasonable default like 5s)
- Whether /v1/models response needs pagination handling (unlikely for mesh-llm)
- Exact log message formatting for discovery success/failure
- Whether to update config.example.toml in the arbstr repo as well

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MESH-01 | mesh-llm endpoint configurable as a standard provider with tier=local and zero-cost rates | ProviderConfig already supports `tier = "local"` and zero-cost `input_rate = 0` / `output_rate = 0`. Just needs config example. [VERIFIED: src/config.rs ProviderConfig struct] |
| MESH-02 | Core polls mesh-llm /v1/models to auto-populate available models on startup | New `auto_discover` field + startup discovery function. mesh-llm returns standard OpenAI `/v1/models` format. [VERIFIED: mesh-llm GitHub README, arbstr server.rs startup flow] |
| MESH-03 | Docker Compose core service can reach mesh-llm on host via extra_hosts configuration | Already configured: `extra_hosts: host.docker.internal:host-gateway` in docker-compose.yml. [VERIFIED: arbstr-node docker-compose.yml line 101-102] |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| reqwest | (existing) | HTTP GET to /v1/models | Already in AppState, no new dependency [VERIFIED: src/proxy/server.rs] |
| serde_json | (existing) | Parse /v1/models response | Already used throughout codebase [VERIFIED: Cargo.toml] |
| serde | (existing) | `#[serde(default)]` for auto_discover field | Already used for all config structs [VERIFIED: src/config.rs] |
| tracing | (existing) | Log discovery success/failure/timeout | Already used for all startup diagnostics [VERIFIED: src/proxy/server.rs] |

### Supporting
No new dependencies required. This phase uses only existing crates. [VERIFIED: codebase analysis]

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Startup-only poll | Background polling with interval | Adds complexity, D-01 explicitly locks startup-only |
| Generic auto_discover | mesh-llm-specific provider type | D-04 locks generic approach, more reusable |

**Installation:**
```bash
# No new dependencies needed
```

## Architecture Patterns

### Discovery Integration Point

The discovery step fits naturally into `run_server()` in `server.rs`, between HTTP client creation (line 174) and `ProviderRouter::new()` (line 167). The flow becomes:

```
Config loaded -> HTTP client created -> DISCOVER MODELS -> ProviderRouter::new() -> AppState -> serve
```

[VERIFIED: src/proxy/server.rs lines 163-235]

### Pattern 1: Startup Model Discovery

**What:** An async function that mutates `config.providers` in place before the router is constructed.
**When to use:** Called once during server startup, never again.
**Example:**
```rust
// Source: recommended pattern based on codebase analysis
use reqwest::Client;
use crate::config::ProviderConfig;
use std::time::Duration;

/// Response from /v1/models (OpenAI-compatible)
#[derive(serde::Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(serde::Deserialize)]
struct ModelEntry {
    id: String,
}

/// Discover models for providers with auto_discover enabled.
/// Mutates provider configs in place, replacing models list with discovered models.
/// On failure, falls back to statically configured models (or empty if none configured).
pub async fn discover_models(providers: &mut [ProviderConfig], client: &Client) {
    for provider in providers.iter_mut() {
        if !provider.auto_discover {
            continue;
        }

        let url = format!("{}/models", provider.url.trim_end_matches('/'));
        tracing::info!(provider = %provider.name, url = %url, "Discovering models");

        match client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<ModelsResponse>().await {
                    Ok(models_resp) => {
                        let model_ids: Vec<String> =
                            models_resp.data.into_iter().map(|m| m.id).collect();
                        let count = model_ids.len();
                        tracing::info!(
                            provider = %provider.name,
                            models = ?model_ids,
                            count = count,
                            "Discovered models"
                        );
                        provider.models = model_ids;
                    }
                    Err(e) => {
                        tracing::warn!(
                            provider = %provider.name,
                            error = %e,
                            "Failed to parse /v1/models response, keeping static models"
                        );
                    }
                }
            }
            Ok(resp) => {
                tracing::warn!(
                    provider = %provider.name,
                    status = %resp.status(),
                    "Discovery endpoint returned non-success status, keeping static models"
                );
            }
            Err(e) => {
                tracing::warn!(
                    provider = %provider.name,
                    error = %e,
                    "Failed to reach discovery endpoint, keeping static models"
                );
            }
        }
    }
}
```

### Pattern 2: Config Field Addition

**What:** Add `auto_discover: bool` to `ProviderConfig` with `#[serde(default)]` for backward compatibility.
**When to use:** Standard pattern already used for `tier`, `base_fee`, etc.
**Example:**
```rust
// Source: existing pattern in src/config.rs
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub url: String,
    pub api_key: Option<ApiKey>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub input_rate: u64,
    #[serde(default)]
    pub output_rate: u64,
    #[serde(default)]
    pub base_fee: u64,
    #[serde(default)]
    pub tier: Tier,
    /// When true, poll /v1/models on startup to discover available models.
    /// Discovered models replace the static `models` list.
    /// If discovery fails, falls back to the static list (or empty).
    #[serde(default)]
    pub auto_discover: bool,
}
```

### Pattern 3: mesh-llm Config Template (arbstr-node)

**What:** Commented-out provider block in arbstr-node's config.toml.
**Example:**
```toml
# mesh-llm local inference (uncomment when mesh-llm is running on host)
# [[providers]]
# name = "mesh-local"
# url = "http://host.docker.internal:9337/v1"
# auto_discover = true
# tier = "local"
# input_rate = 0
# output_rate = 0
```

### Anti-Patterns to Avoid
- **Mesh-llm-specific provider type:** D-04 locks the generic approach. Don't add a `type` enum or mesh-llm-specific logic. [VERIFIED: CONTEXT.md D-04]
- **Background model refresh:** D-01 locks startup-only. Don't add tokio::spawn for periodic polling. [VERIFIED: CONTEXT.md D-01]
- **Model name normalization:** D-06 locks exact names. Don't strip quantization suffixes or add fuzzy matching. [VERIFIED: CONTEXT.md D-06]
- **Blocking startup on discovery failure:** D-02 requires non-blocking. Discovery failures must log and continue. [VERIFIED: CONTEXT.md D-02]

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| HTTP GET with timeout | Raw TCP/manual timeout | `reqwest::Client::get().timeout()` | Already available in AppState [VERIFIED: server.rs] |
| JSON deserialization | Manual parsing | `serde_json` / `resp.json::<T>()` | Type-safe, handles edge cases [VERIFIED: codebase pattern] |
| OpenAI models response | Custom response types | Standard `{data: [{id: String}]}` struct | OpenAI spec is stable, mesh-llm conforms [VERIFIED: mesh-llm docs] |

## Common Pitfalls

### Pitfall 1: Discovery Timeout Blocks Startup
**What goes wrong:** If mesh-llm is configured but not running, the default reqwest timeout (120s from server.rs line 175) would delay startup by 2 minutes per unreachable provider.
**Why it happens:** The `http_client` in AppState has a 120-second timeout. Discovery reuses this client.
**How to avoid:** Use a per-request `.timeout(Duration::from_secs(5))` on the discovery GET call, overriding the client default. 5 seconds is generous for a localhost service. [VERIFIED: reqwest supports per-request timeout override]
**Warning signs:** Slow startup when mesh-llm is offline.

### Pitfall 2: Model Names With Quantization Suffixes
**What goes wrong:** mesh-llm returns full quantized names like `Qwen3-8B-Q4_K_M` from `/v1/models`. Users might configure clients to request `Qwen3-8B` (the base name), which won't match. [CITED: .planning/research/PITFALLS.md Pitfall 5]
**Why it happens:** mesh-llm uses llama.cpp model names which include GGUF quantization variants.
**How to avoid:** D-06 locks exact-name behavior (no normalization). The `/v1/models` endpoint (D-07) lets clients discover exact names. Document this in config comments.
**Warning signs:** "No providers for model" errors when model base names are used without quantization suffix.

### Pitfall 3: URL Trailing Slash Mismatch
**What goes wrong:** If `provider.url = "http://host.docker.internal:9337/v1/"` (trailing slash), concatenating `/models` produces `http://host.docker.internal:9337/v1//models` (double slash). Some servers reject this.
**Why it happens:** Inconsistent URL normalization in config.
**How to avoid:** `provider.url.trim_end_matches('/')` before appending `/models`. [VERIFIED: common Rust HTTP pattern]
**Warning signs:** 404 errors during discovery.

### Pitfall 4: Empty Models List After Failed Discovery
**What goes wrong:** If `auto_discover = true` and `models = []` (no static fallback), and discovery fails, the provider has zero models. It matches no requests but still appears in `/health` and `/providers`. Users may be confused why their provider is "connected" but serving nothing.
**Why it happens:** D-02 specifies non-blocking startup with zero models on failure.
**How to avoid:** Log a clear warning: "Provider 'mesh-local' has no models after failed discovery. It won't match any requests until restarted with the provider available." [VERIFIED: CONTEXT.md D-02]
**Warning signs:** Provider appears healthy in `/health` but never receives traffic.

### Pitfall 5: Docker host.docker.internal Not Available on Linux Without extra_hosts
**What goes wrong:** `host.docker.internal` is native on Docker Desktop (macOS/Windows) but requires `extra_hosts: host.docker.internal:host-gateway` on Linux. Without it, DNS resolution fails inside the container.
**Why it happens:** Docker Desktop includes a special DNS entry; Linux Docker engine does not.
**How to avoid:** Already handled -- arbstr-node's docker-compose.yml includes `extra_hosts` on the core service. [VERIFIED: arbstr-node docker-compose.yml lines 101-102]
**Warning signs:** Connection refused or DNS resolution failure when core tries to reach mesh-llm.

## Code Examples

### Integration into run_server()

```rust
// Source: recommended insertion point in src/proxy/server.rs
// After line 177 (http_client creation), before line 167 (ProviderRouter::new)

pub async fn run_server(mut config: Config) -> anyhow::Result<()> {
    let listen_addr = config.server.listen.clone();

    // Create HTTP client with reasonable defaults
    let http_client = Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    // NEW: Discover models for auto_discover providers
    discover_models(&mut config.providers, &http_client).await;

    // Create provider router (now with discovered models)
    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );
    // ... rest unchanged
```

Note: `run_server` signature changes from `config: Config` to `mut config: Config` to allow mutation. [VERIFIED: current signature at server.rs line 163]

### OpenAI /v1/models Response Format

mesh-llm returns standard OpenAI format. [CITED: github.com/michaelneale/mesh-llm]

```json
{
  "object": "list",
  "data": [
    {
      "id": "Qwen3-8B-Q4_K_M",
      "object": "model",
      "owned_by": "mesh-llm"
    },
    {
      "id": "GLM-4.7-Flash-Q4_K_M",
      "object": "model",
      "owned_by": "mesh-llm"
    }
  ]
}
```

Only the `id` field is needed for discovery. The `data` array and `id` fields are the stable contract. [VERIFIED: OpenAI API spec, mesh-llm docs]

### Test Pattern for Discovery

```rust
// Source: recommended test pattern
#[tokio::test]
async fn test_discover_models_success() {
    // Start a mock server that returns /v1/models
    let mock_server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v1/models"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {"id": "test-model-1", "object": "model"},
                {"id": "test-model-2", "object": "model"}
            ]
        })))
        .mount(&mock_server)
        .await;

    let mut providers = vec![ProviderConfig {
        name: "test".to_string(),
        url: format!("{}/v1", mock_server.uri()),
        auto_discover: true,
        models: vec!["fallback".to_string()],
        ..Default::default()
    }];

    let client = reqwest::Client::new();
    discover_models(&mut providers, &client).await;

    assert_eq!(providers[0].models, vec!["test-model-1", "test-model-2"]);
}
```

Note: `wiremock` crate may be needed for unit testing discovery. Check if it's already in dev-dependencies. [ASSUMED]

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Static model lists in config | Auto-discovery via /v1/models | This phase | Eliminates model name mismatch for local providers |
| mesh-llm-specific provider type | Generic auto_discover flag | D-04 decision | Any OpenAI-compatible local provider can use discovery |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | wiremock crate may be needed for unit testing discovery | Code Examples (test pattern) | LOW -- could use existing test mock infrastructure or add as dev-dependency |
| A2 | mesh-llm /v1/models response includes `object` and `owned_by` fields beyond `id` | Code Examples (response format) | NONE -- only `id` is consumed, extra fields ignored by serde |

## Open Questions

1. **Should config.example.toml in the arbstr repo also be updated?**
   - What we know: CONTEXT.md lists this as Claude's discretion. arbstr-node config.toml gets the mesh-llm example (D-08/D-09).
   - What's unclear: Whether to add auto_discover to the arbstr repo's example config as well.
   - Recommendation: Yes -- add `auto_discover = false` (commented) to config.example.toml provider entries so users discover the feature. Add a commented mesh-llm example block too.

2. **Should ProviderConfig derive Default?**
   - What we know: Currently it does not derive Default. Tests construct it manually with all fields.
   - What's unclear: Whether adding `#[derive(Default)]` or a manual Default impl would simplify test code.
   - Recommendation: Not needed for this phase -- the new `auto_discover` field uses `#[serde(default)]` which is sufficient. Test code already constructs full structs.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test + tokio::test (async) |
| Config file | Cargo.toml `[dev-dependencies]` |
| Quick run command | `cargo test --lib discover` |
| Full suite command | `cargo test` |

### Phase Requirements -> Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MESH-01 | mesh-llm configurable as provider with tier=local, zero-cost | unit | `cargo test --lib -- provider_config` | Existing selector tests cover tier/cost |
| MESH-02 | Startup polls /v1/models, populates model list | unit + integration | `cargo test -- discover` | Wave 0 (new) |
| MESH-03 | Docker Compose reaches mesh-llm on host | manual | `docker compose up core` with mesh-llm running | Manual-only (requires host services) |

### Sampling Rate
- **Per task commit:** `cargo test --lib`
- **Per wave merge:** `cargo test`
- **Phase gate:** Full suite green before verify

### Wave 0 Gaps
- [ ] Unit test for `discover_models()` success path (mock server returns models)
- [ ] Unit test for `discover_models()` failure path (unreachable provider keeps static models)
- [ ] Unit test for `discover_models()` with `auto_discover = false` (skip discovery)
- [ ] Config deserialization test for `auto_discover` field (default false, explicit true)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | Discovery is unauthenticated GET to local endpoint |
| V3 Session Management | no | N/A |
| V4 Access Control | no | N/A |
| V5 Input Validation | yes | Validate /v1/models response structure (deserialize to typed struct, reject malformed) |
| V6 Cryptography | no | N/A |

### Known Threat Patterns

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Malicious /v1/models response with oversized model list | Denial of Service | Set reasonable limit on models array size (e.g., 1000) or use bounded deserialization [ASSUMED] |
| SSRF via auto_discover to internal network | Spoofing/Information Disclosure | Config file is trusted input controlled by operator; no user-supplied URLs [VERIFIED: config.rs] |

Security risk is LOW for this phase. Discovery only makes unauthenticated GET requests to operator-configured URLs, parsing a small JSON response. No secrets are transmitted, no user input drives the URL.

## Sources

### Primary (HIGH confidence)
- `src/config.rs` -- ProviderConfig struct (line 262), Tier enum (line 158), serde patterns
- `src/proxy/server.rs` -- run_server() startup flow, AppState construction, HTTP client creation
- `src/proxy/handlers.rs` -- list_models handler (line 1521), model aggregation logic
- `src/router/selector.rs` -- Router::new(), provider filtering by models list
- `arbstr-node/docker-compose.yml` -- extra_hosts configuration (line 101-102)
- `arbstr-node/config.toml` -- existing provider config template
- `.planning/phases/24-mesh-llm-provider/24-CONTEXT.md` -- all locked decisions D-01 through D-09

### Secondary (MEDIUM confidence)
- [mesh-llm GitHub](https://github.com/michaelneale/mesh-llm) -- /v1/models endpoint format, port 9337, model naming conventions
- `.planning/research/PITFALLS.md` -- Pitfall 5 (model name mismatch), Pitfall 6 (node disappears)
- `.planning/research/ARCHITECTURE.md` -- System topology, mesh-llm integration pattern

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- no new dependencies, all verified in codebase
- Architecture: HIGH -- insertion point clearly identified in server.rs, pattern matches existing codebase conventions
- Pitfalls: HIGH -- based on direct codebase analysis and prior pitfalls research

**Research date:** 2026-04-10
**Valid until:** 2026-05-10 (stable domain, no fast-moving dependencies)
