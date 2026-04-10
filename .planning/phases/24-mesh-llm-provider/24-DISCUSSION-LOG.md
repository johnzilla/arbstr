# Phase 24: mesh-llm Provider - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-10
**Phase:** 24-mesh-llm-provider
**Areas discussed:** Model discovery mechanism, Provider type flag, Model name handling, Compose config template

---

## Model Discovery Mechanism

| Option | Description | Selected |
|--------|-------------|----------|
| Startup poll only | GET /v1/models once at startup, populate model list, done. Simple, no background tasks. | ✓ |
| Periodic refresh | Poll /v1/models every N seconds. Picks up new models without restart. | |
| Startup + on circuit recovery | Poll at startup and re-poll when circuit breaker recovers. | |

**User's choice:** Startup poll only
**Notes:** None — straightforward preference for simplicity.

### Follow-up: Unreachable at startup

| Option | Description | Selected |
|--------|-------------|----------|
| Warn and keep going | Log warning, keep any config models, provider sits idle. Non-blocking. | ✓ |
| Retry with backoff | Retry 3 times with exponential backoff before giving up. | |
| Block startup | Fail to start if provider is unreachable. | |

**User's choice:** Warn and keep going
**Notes:** Non-blocking startup preferred.

---

## Provider Type Flag

| Option | Description | Selected |
|--------|-------------|----------|
| auto_discover = true | Generic boolean on ProviderConfig. Any provider can opt in. Default: false. | ✓ |
| type = "mesh-llm" | Enum field triggering mesh-specific behavior. More semantic but less generic. | |
| Empty models = auto-discover | Convention: empty models list triggers discovery. Ambiguous. | |

**User's choice:** auto_discover = true
**Notes:** Generic approach preferred — works for Ollama, any OpenAI-compatible endpoint.

---

## Model Name Handling

| Option | Description | Selected |
|--------|-------------|----------|
| Use exact names | Register models exactly as reported. Clients must use exact names. | ✓ |
| Register both exact + base alias | Register full name + base name. Client can use either. | |
| You decide | Let Claude choose during planning. | |

**User's choice:** Use exact names
**Notes:** Clean and unambiguous. arbstr's /v1/models endpoint provides discoverability.

---

## Compose Config Template

| Option | Description | Selected |
|--------|-------------|----------|
| Commented-out example | Ship mesh-llm entry commented out. Users uncomment when ready. | ✓ |
| Active by default | Ship enabled. Discovery warns if unreachable, provider sits idle. | |
| Separate config file | Ship a mesh-llm.toml snippet for manual merging. | |

**User's choice:** Commented-out example
**Notes:** Avoids startup warnings when mesh-llm isn't installed.

---

## Claude's Discretion

- How to structure startup discovery code
- Discovery request timeout
- /v1/models pagination handling
- Log message formatting
- Whether to also update config.example.toml in arbstr repo

## Deferred Ideas

None — discussion stayed within phase scope
