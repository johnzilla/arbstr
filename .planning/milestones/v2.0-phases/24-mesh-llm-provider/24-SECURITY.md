---
phase: 24-mesh-llm-provider
plan: 01
asvs_level: 1
threats_total: 4
threats_closed: 4
threats_open: 0
status: SECURED
---

# Security Verification — Phase 24: Mesh LLM Provider

**Phase:** 24 — mesh-llm-provider
**Threats Closed:** 4/4
**ASVS Level:** 1

## Threat Verification

| Threat ID | Category | Disposition | Status | Evidence |
|-----------|----------|-------------|--------|----------|
| T-24-01 | Denial of Service | mitigate | CLOSED | `src/proxy/discovery.rs:33` — `.timeout(Duration::from_secs(5))` on each discovery request |
| T-24-02 | Information Disclosure (SSRF) | accept | CLOSED | See accepted risks log below |
| T-24-03 | Tampering (malicious model names) | accept | CLOSED | See accepted risks log below |
| T-24-04 | Denial of Service (oversized response) | mitigate | CLOSED | `src/proxy/discovery.rs:33,41` — 5-second timeout bounds body read; `resp.json::<ModelsResponse>()` rejects malformed JSON via serde |

## Mitigation Evidence

### T-24-01: Per-request 5-second discovery timeout

`src/proxy/discovery.rs` line 33:
```rust
let mut request = client.get(&url).timeout(Duration::from_secs(5));
```
The `.timeout()` call is applied per-request before `.send()`. A slow or hung provider endpoint will be abandoned after 5 seconds, preventing it from blocking server startup indefinitely.

### T-24-04: Time-bounded body consumption + serde rejection

The same 5-second timeout at line 33 governs both connection and the full response body read (reqwest applies the timeout to the entire request lifecycle). `resp.json::<ModelsResponse>()` at line 41 deserializes into a typed struct; any malformed JSON or unexpected shape returns `Err` and triggers the graceful-degradation path (warning logged, static models retained). There is no explicit byte cap — the mitigation is time-based per the plan's accepted scope for a localhost-tier service.

## Accepted Risks Log

### T-24-02 — Information Disclosure / SSRF via discover_models

**Accepted by:** operator (config owner)
**Rationale:** Provider URLs in `auto_discover` providers are sourced exclusively from operator-controlled `config.toml`. This is the same trust boundary that already governs all inference forwarding URLs in existing providers. The `auto_discover` field does not introduce a new URL-injection surface — it is structurally identical to existing provider URL handling. No user-supplied input can influence which URLs are polled.
**Residual risk:** An operator who misconfigures a URL could cause arbstr to make a startup HTTP GET to an unintended internal host. This is within the operator's own trust domain.

### T-24-03 — Tampering via malicious /v1/models response

**Accepted by:** operator (config owner)
**Rationale:** Discovered model names are strings stored in `ProviderConfig.models` and used only for substring/equality matching in the router (`src/router/selector.rs`). There is no code execution, shell interpolation, SQL query construction, or filesystem access performed on model name strings. The worst-case outcome of a tampered model list is an incorrect model appearing in `/v1/models` output or a routing mismatch. Operators control which providers have `auto_discover = true`.
**Residual risk:** A compromised provider could advertise misleading model names, potentially causing requests to be routed to it that the operator intended for another provider. Operators should only enable `auto_discover` on trusted providers.

## Unregistered Threat Flags

None. SUMMARY.md contains no `## Threat Flags` section.
