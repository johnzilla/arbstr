# Technology Stack: Secrets Handling for arbstr

**Project:** arbstr - Env var expansion in config and API key redaction
**Researched:** 2026-02-15
**Overall confidence:** HIGH

## Scope

This research covers ONLY the stack additions needed for:
1. Env var expansion in TOML config values (e.g., `api_key = "${ROUTSTR_KEY}"`)
2. Convention-based env var lookup (e.g., `ARBSTR_PROVIDER_ALPHA_API_KEY`)
3. Secret redaction in Debug output, logs, and the `/providers` endpoint

Everything else in the existing stack is unchanged and validated from v1.

## Existing Stack (No Changes Needed)

These dependencies remain correct and require no modifications:

| Technology | Version | Purpose | Status |
|------------|---------|---------|--------|
| tokio | 1.x (full) | Async runtime | Keep as-is |
| axum | 0.7 | HTTP server | Keep as-is |
| reqwest | 0.12 | HTTP client | Keep as-is |
| serde / serde_json | 1.x | Serialization | Keep as-is |
| toml | 0.8 | TOML parsing | Keep as-is (config parsing stays with toml crate) |
| tracing / tracing-subscriber | 0.1 / 0.3 | Structured logging | Keep as-is |
| thiserror | 1.x | Error types | Keep as-is |

## Dependency to Remove

### `config` crate (currently `config = "0.14"`)

**Action:** Remove from `Cargo.toml`.

**Why:** This dependency is declared but completely unused. The project parses config exclusively via `toml::from_str()` in `config.rs`. No code imports or references `config::*` from this crate. Removing it eliminates a transitive dependency tree and avoids confusion with the project's own `crate::config` module.

**Could we use it instead?** The `config` crate does support `Environment::with_prefix("ARBSTR")` for env var overrides, but it would require rewriting the entire config loading pipeline to use its builder pattern. The current approach (toml + post-processing) is simpler, more explicit, and already works. Not worth the migration cost.

## New Dependencies Required

### 1. Secret Wrapper: `secrecy` 0.10

| Technology | Version | Purpose | Confidence |
|------------|---------|---------|------------|
| secrecy | 0.10 | Wrap API keys so Debug/Display auto-redact | HIGH |

```toml
secrecy = { version = "0.10", features = ["serde"] }
```

**What it does:** Provides `SecretString` (type alias for `SecretBox<str>`) that:
- Implements `Debug` as `SecretString([REDACTED])` -- prevents accidental key leakage in logs
- Implements `Deserialize` (with `serde` feature) so it works directly in serde structs
- Does NOT implement `Serialize` by default -- prevents accidental serialization of secrets
- Zeroizes memory on drop via the `zeroize` crate

**Why secrecy specifically:**
- De facto standard for secret handling in Rust (maintained by iqlusion, used by major projects)
- Minimal API surface: `SecretString`, `ExposeSecret` trait, done
- Integrates with the existing serde deserialization pipeline with zero config changes
- `Option<SecretString>` works out of the box with serde (missing TOML field = `None`)
- No runtime overhead beyond the zeroize-on-drop

**How it integrates with existing code:**

The `api_key` field in `ProviderConfig` changes from `Option<String>` to `Option<SecretString>`:

```rust
use secrecy::SecretString;

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub url: String,
    pub api_key: Option<SecretString>,  // was Option<String>
    // ... rest unchanged
}
```

Access points that need the raw value use `expose_secret()`:

```rust
use secrecy::ExposeSecret;

// In handlers.rs, when setting the Authorization header:
if let Some(api_key) = &provider.api_key {
    upstream_request = upstream_request
        .header(header::AUTHORIZATION, format!("Bearer {}", api_key.expose_secret()));
}
```

The `SelectedProvider` struct similarly changes its `api_key` field. Since both structs derive `Debug`, any tracing log that includes `{:?}` on a provider will now automatically show `[REDACTED]` instead of the key value.

**Clone consideration:** `SecretString` (i.e., `SecretBox<str>`) implements `Clone`. The existing code clones `ProviderConfig` and `SelectedProvider` extensively. This works without changes.

**Alternatives considered:**

| Crate | Why Not |
|-------|---------|
| `redact` | Newer, less battle-tested. Focuses on partial redaction (show last N chars) which is nice but not essential. secrecy is the ecosystem standard. |
| `redaction` | Very new crate. Overkill policy engine for our use case. |
| `safelog` | Thread-local global toggle model. Doesn't fit our per-field secret wrapping. |
| Hand-rolled newtype | Works but reinvents the wheel. No zeroize-on-drop. No ecosystem recognition. |

### 2. No New Dependency for Env Var Expansion

**Action:** Implement env var expansion as a ~30-line post-processing function in `config.rs`. Do NOT add a crate for this.

**Why no crate:**

| Crate | Downloads | Why Not |
|-------|-----------|---------|
| `shellexpand` 3.1 | Popular | Handles `~` home dir, `$VAR`, `${VAR}`. Overkill -- we only need `${VAR}` in specific string fields, not shell expansion. Also handles tilde which is wrong for API keys. |
| `serde-env-field` | Low adoption | Wraps every field in `EnvField<T>`. Invasive to struct definitions. We only need expansion on string fields that might contain secrets. |
| `serde-with-expand-env` | Very low adoption | `#[serde(deserialize_with)]` approach. Ties expansion to deserialization -- can't control expansion timing or error reporting separately. |
| `envsubst` | Low adoption | Simple but another dependency for 30 lines of code. |

**The implementation is trivial:**

```rust
use std::env;

/// Expand `${VAR_NAME}` patterns in a string with environment variable values.
/// Returns the expanded string, or an error if a referenced variable is not set.
fn expand_env_vars(input: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let var_name: String = chars.by_ref().take_while(|&c| c != '}').collect();
            if var_name.is_empty() {
                return Err(ConfigError::Validation("Empty variable name in ${}".into()));
            }
            match env::var(&var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => return Err(ConfigError::Validation(
                    format!("Environment variable '{}' is not set", var_name)
                )),
            }
        } else {
            result.push(c);
        }
    }
    Ok(result)
}
```

This function runs as a post-processing step after `toml::from_str()` but before validation. It only processes fields that contain `${...}` syntax. Simple, testable, no dependencies, no surprises.

**Convention-based env var fallback** (e.g., `ARBSTR_PROVIDER_ALPHA_API_KEY`) is also pure std::env logic -- ~10 lines in the config loading pipeline. No crate needed.

## Integration Architecture

### Config Loading Pipeline (Modified)

The current pipeline is:
```
TOML file -> toml::from_str() -> Config struct -> validate() -> done
```

The new pipeline becomes:
```
TOML file -> toml::from_str() -> Config struct
  -> expand_env_vars() on string fields containing "${...}"
  -> convention env var fallback for missing api_keys
  -> validate()
  -> done
```

The key design decision: **expand env vars AFTER deserialization, not during.** This keeps the serde layer clean, gives explicit control over which fields get expanded, and produces clear error messages with field context ("Provider 'alpha' api_key references undefined env var 'ROUTSTR_KEY'").

### Where Secrets Flow Through the System

Understanding the flow is critical for knowing where `expose_secret()` calls are needed:

```
config.toml                       ProviderConfig.api_key: Option<SecretString>
    |                                      |
    v                                      v
Config::from_file()               Router stores Vec<ProviderConfig>
    |                                      |
    v                                      v
ProviderConfig.api_key            SelectedProvider.api_key: Option<SecretString>
    |                                      |
    v                                      v (only place value is exposed)
expand_env_vars()                 handlers.rs: expose_secret() for Authorization header
    |
    v
Convention fallback               /providers endpoint: api_key NOT included (already the case)
```

Total `expose_secret()` call sites: **1** (the Authorization header in `send_to_provider`). Everything else works with the redacted wrapper.

### Debug/Log Redaction (Automatic)

Once `api_key` is `SecretString`, these existing log statements automatically redact:

- `tracing::info!(provider = ?provider, ...)` -- Debug on ProviderConfig now redacts api_key
- `tracing::error!(... provider = %provider.name, ...)` -- These log `name` not the full struct, already safe
- Any `format!("{:?}", config)` -- Config Debug output now redacts all api_keys

No changes to existing tracing calls needed. The type system does the work.

### /providers Endpoint (Already Safe)

The `list_providers` handler in `handlers.rs` already constructs its JSON response manually and does NOT include `api_key`:

```rust
serde_json::json!({
    "name": p.name,
    "models": p.models,
    "input_rate_sats_per_1k": p.input_rate,
    "output_rate_sats_per_1k": p.output_rate,
    "base_fee_sats": p.base_fee,
    // no api_key field
})
```

No changes needed here. But the `SecretString` type provides defense-in-depth: even if someone later adds `api_key` to this JSON, `SecretString` does not implement `Serialize` by default, so it would fail to compile. This is a feature, not a bug.

## Cargo.toml Changes Summary

```toml
[dependencies]
# ADD:
secrecy = { version = "0.10", features = ["serde"] }

# REMOVE:
# config = "0.14"  # unused dependency
```

Net dependency change: +1 (secrecy), -1 (config). The `secrecy` crate pulls in `zeroize` as its only meaningful transitive dependency, which is tiny.

## What NOT to Add

| Technology | Why Not |
|------------|---------|
| `dotenv` / `dotenvy` | The project loads config from TOML, not .env files. Env vars are the system's job. Users can use direnv, systemd EnvironmentFile, or shell scripts. Adding .env support creates a second config surface. |
| `vault` / `aws-secretsmanager` | External secret managers are out of scope. This is a local proxy. Env vars are the right abstraction -- they work with any secret manager via the shell. |
| `ring` / `aes-gcm` | Encrypting secrets at rest in the config file is over-engineering. The config file is local. File permissions are the right control. |
| `tracing-sensitive` | No such established crate. The secrecy type handles this at the data layer, which is more reliable than log filtering. |
| `regex` | The env var expansion pattern `${VAR}` is simple enough to parse with a char iterator. Regex is overkill and adds a large dependency. |

## Version Verification

| Crate | Version | Verified Via | Confidence |
|-------|---------|-------------|------------|
| secrecy | 0.10.3 | crates.io search, docs.rs | HIGH |
| secrecy serde feature | Available in 0.10.x | docs.rs API docs, forum posts | HIGH |
| shellexpand | 3.1.x (not using) | crates.io search | HIGH (verified to reject) |

## Sources

- [secrecy crate on crates.io](https://crates.io/crates/secrecy) -- version 0.10.3, serde feature documented
- [secrecy API documentation on docs.rs](https://docs.rs/secrecy/latest/secrecy/) -- SecretString, ExposeSecret trait, serde integration
- [SecretString type docs](https://docs.rs/secrecy/latest/secrecy/type.SecretString.html) -- type alias for SecretBox<str>, Deserialize impl
- [Secure Configuration and Secrets Management in Rust with Secrecy](https://leapcell.io/blog/secure-configuration-and-secrets-management-in-rust-with-secrecy-and-environment-variables) -- practical usage patterns
- [Secrecy Crate: Serialize String discussion](https://users.rust-lang.org/t/secrecy-crate-serialize-string/112263) -- serialization constraints are intentional
- [shellexpand crate on crates.io](https://crates.io/crates/shellexpand) -- version 3.1.x, feature scope (tilde + env vars)
- [serde-env-field on lib.rs](https://lib.rs/crates/serde-env-field) -- EnvField wrapper approach
- [serde-with-expand-env on docs.rs](https://docs.rs/serde-with-expand-env/latest/serde_with_expand_env/) -- deserialize_with approach
- [config crate Environment docs](https://docs.rs/config/latest/config/struct.Environment.html) -- prefix-based env var overrides
- [redaction crate on GitHub](https://github.com/sformisano/redaction) -- alternative redaction library
