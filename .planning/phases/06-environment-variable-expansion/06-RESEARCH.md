# Phase 6: Environment Variable Expansion - Research

**Researched:** 2026-02-15
**Domain:** Config-time environment variable expansion (`${VAR}` syntax), convention-based key auto-discovery (`ARBSTR_<NAME>_API_KEY`), key source reporting
**Confidence:** HIGH

## Summary

This phase adds two mechanisms for resolving API keys from environment variables, plus observability into which mechanism provided each key. The first mechanism is **explicit expansion**: when a config value contains `${VAR_NAME}`, the referenced environment variable is substituted at config load time. The second mechanism is **convention-based auto-discovery**: when `api_key` is omitted from a provider's config, arbstr checks for `ARBSTR_<UPPER_SNAKE_NAME>_API_KEY` in the environment. Both mechanisms produce an `ApiKey` value that feeds into the existing `Option<ApiKey>` field on `ProviderConfig`.

The implementation follows a **two-phase config loading** pattern: (1) parse TOML into a raw `Config` struct where `api_key` is `Option<String>`, (2) expand env vars on the raw strings, (3) apply convention-based lookup for providers still missing keys, (4) convert expanded strings into `ApiKey` values. This approach was established as a prior research decision and integrates cleanly with the existing `ApiKey(SecretString)` type from Phase 5.

No new crate dependencies are needed. The `${VAR}` pattern matching can be implemented with a simple `find("${")` / `find("}")` loop on `&str`, and env var lookup uses `std::env::var()` from the standard library. The regex crate is not in the current dependency list and adding it for a single simple pattern is unnecessary overhead.

**Primary recommendation:** Introduce a `RawProviderConfig` struct with `api_key: Option<String>`, parse TOML into that, run env var expansion and convention lookup, then convert to `ProviderConfig` with `api_key: Option<ApiKey>`. Add a `KeySource` enum (`Literal`, `EnvExpanded`, `Convention`, `None`) to track provenance per provider, logged at startup and reported by the `check` command.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| std::env::var | stdlib | Look up individual env vars by name | No external dependency needed for simple key-value lookup |
| secrecy | 0.10 (already present) | Wrap resolved keys in SecretString | Already in project from Phase 5 |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| toml | 0.8 (already present) | Parse TOML into raw config | Already used for config parsing |
| tracing | 0.1 (already present) | Log key source per provider at startup | Already used throughout |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Hand-rolled `${VAR}` expansion | `shellexpand` crate (3.1.1, 2M downloads/week) | shellexpand adds `$VAR` (without braces), `~` expansion, and default values (`${VAR:-default}`) -- overkill for this use case, adds `dirs` dependency; prior decision says no new crates |
| Hand-rolled `${VAR}` expansion | `envsubst` crate (0.2.1) | Takes explicit HashMap context instead of reading env directly; low adoption; prior decision says no new crates |
| Hand-rolled `${VAR}` expansion | `regex` crate for pattern matching | regex is not in current deps; `find("${")` loop is simpler for this one pattern; prior decision says stdlib is sufficient |

**No new dependencies needed.** The prior research decision explicitly states: "No new crates for env expansion -- stdlib std::env::var is sufficient."

## Architecture Patterns

### Two-Phase Config Loading

**What:** Parse TOML into a raw intermediate struct, process env vars, then convert to the final `Config` with `ApiKey` types.
**When to use:** When config values need runtime transformation (env var expansion) before being wrapped in opaque types (SecretString).
**Why:** The current `ApiKey::deserialize` impl calls `String::deserialize(d).map(|s| ApiKey(SecretString::from(s)))`, which means the string is immediately wrapped. Env var expansion must happen on the raw string BEFORE wrapping. The two-phase approach cleanly separates "parse what the user wrote" from "resolve what the user meant."

```
                           TOML file
                               |
                         [Phase 1: Parse]
                               |
                     RawConfig (api_key: Option<String>)
                               |
                      [Phase 2: Expand]
                         expand_env_vars("${MY_KEY}") -> "cashuA..."
                               |
                      [Phase 3: Convention lookup]
                         if api_key is None, check ARBSTR_<NAME>_API_KEY
                               |
                      [Phase 4: Convert]
                         String -> ApiKey(SecretString)
                               |
                     Config (api_key: Option<ApiKey>)
```

### Recommended Implementation Structure

```
src/
├── config.rs              # Add: RawConfig, RawProviderConfig, expand_env_vars(),
│                          #       convention_key_lookup(), KeySource enum,
│                          #       Config::from_raw() conversion method
├── main.rs                # Update: serve/check/providers commands use new loading
├── error.rs               # Add: EnvVar error variant
├── proxy/                 # No changes (receives Config with resolved keys)
└── router/                # No changes (receives ProviderConfig with resolved keys)
```

### Pattern 1: Raw Config Struct

**What:** A parallel struct hierarchy where secret fields are plain `String` instead of `ApiKey`.
**When to use:** For the deserialization target before env var processing.

```rust
/// Raw configuration deserialized directly from TOML.
/// api_key values may contain ${VAR} references not yet expanded.
#[derive(Deserialize)]
struct RawConfig {
    server: ServerConfig,
    database: Option<DatabaseConfig>,
    #[serde(default)]
    providers: Vec<RawProviderConfig>,
    #[serde(default)]
    policies: PoliciesConfig,
    #[serde(default)]
    logging: LoggingConfig,
}

/// Raw provider config with api_key as plain String (pre-expansion).
#[derive(Deserialize)]
struct RawProviderConfig {
    name: String,
    url: String,
    api_key: Option<String>,  // "${MY_KEY}" or "literal" or absent
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    input_rate: u64,
    #[serde(default)]
    output_rate: u64,
    #[serde(default)]
    base_fee: u64,
}
```

Note: Only `RawProviderConfig` differs from `ProviderConfig`. The other sub-configs (`ServerConfig`, `DatabaseConfig`, `PoliciesConfig`, `LoggingConfig`) can be shared since they have no secret fields. This keeps the duplication minimal.

### Pattern 2: Environment Variable Expansion Function

**What:** A function that finds `${VAR}` patterns in a string and replaces them with env var values.
**When to use:** Applied to raw string values before wrapping in ApiKey.

```rust
/// Expand all ${VAR} references in a string.
/// Returns the expanded string or an error naming the missing variable.
fn expand_env_vars(input: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after_dollar_brace = &rest[start + 2..];

        let end = after_dollar_brace.find('}').ok_or_else(|| {
            ConfigError::Validation(format!(
                "Unclosed '${{' in value: {}",
                input
            ))
        })?;

        let var_name = &after_dollar_brace[..end];
        let value = std::env::var(var_name).map_err(|_| {
            ConfigError::EnvVar {
                var: var_name.to_string(),
                context: input.to_string(),
            }
        })?;
        result.push_str(&value);
        rest = &after_dollar_brace[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}
```

Key design choices:
- Supports multiple `${VAR}` in one value (e.g., `${SCHEME}://${HOST}`)
- Fails immediately on first missing variable (not best-effort)
- Reports which variable is missing (requirement ENV-02)
- Handles `${VAR}` anywhere in the string, not just as the entire value
- No regex needed -- simple `find()` loop

### Pattern 3: Convention-Based Auto-Discovery

**What:** When `api_key` is absent from config, check `ARBSTR_<UPPER_SNAKE_NAME>_API_KEY`.
**When to use:** After explicit expansion, for providers that still have no key.

```rust
/// Derive the convention-based env var name for a provider.
/// "provider-alpha" -> "ARBSTR_PROVIDER_ALPHA_API_KEY"
fn convention_env_var_name(provider_name: &str) -> String {
    let upper_snake = provider_name
        .to_uppercase()
        .replace('-', "_")
        .replace(' ', "_");
    format!("ARBSTR_{}_API_KEY", upper_snake)
}
```

Name transformation rules:
- Uppercase the entire provider name
- Replace hyphens with underscores
- Replace spaces with underscores (defensive)
- Prefix with `ARBSTR_` and suffix with `_API_KEY`

Examples:
| Provider Name | Convention Env Var |
|---------------|-------------------|
| `alpha` | `ARBSTR_ALPHA_API_KEY` |
| `provider-beta` | `ARBSTR_PROVIDER_BETA_API_KEY` |
| `my_service` | `ARBSTR_MY_SERVICE_API_KEY` |

### Pattern 4: Key Source Tracking

**What:** An enum that records how each provider's key was resolved.
**When to use:** For startup logging (ENV-04) and the `check` command (ENV-05).

```rust
/// How a provider's API key was resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum KeySource {
    /// Key was a literal string in config (no ${} references)
    Literal,
    /// Key contained ${VAR} references that were expanded from environment
    EnvExpanded,
    /// Key was auto-discovered from ARBSTR_<NAME>_API_KEY convention
    Convention(String),  // holds the env var name for reporting
    /// No key available (neither in config nor in environment)
    None,
}
```

This enum is returned alongside the resolved `ProviderConfig` and used for:
1. Startup log messages: `provider alpha: key from env-expanded`
2. `check` command output: reporting which vars resolve and which are missing

### Pattern 5: Updated Config Loading Flow

**What:** The new `Config::from_file` pipeline.

```rust
impl Config {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| ConfigError::Io { path: path.as_ref().display().to_string(), source: e })?;
        Self::from_str_with_env(&content)
    }

    /// Parse config, expand env vars, apply conventions, validate.
    /// Returns config and per-provider key source info.
    pub fn from_str_with_env(content: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(content)?;
        let (config, key_sources) = Self::from_raw(raw)?;
        config.validate()?;

        // Log key sources (ENV-04)
        for (provider_name, source) in &key_sources {
            match source {
                KeySource::Literal => tracing::info!(provider = %provider_name, "key from config-literal"),
                KeySource::EnvExpanded => tracing::info!(provider = %provider_name, "key from env-expanded"),
                KeySource::Convention(var) => tracing::info!(provider = %provider_name, env_var = %var, "key from convention"),
                KeySource::None => tracing::warn!(provider = %provider_name, "no api key available"),
            }
        }

        Ok(config)
    }
}
```

### Anti-Patterns to Avoid

- **Expanding env vars inside serde Deserialize:** Serde's deserialize runs during TOML parsing, before you have provider context for error messages. Keep expansion as a separate explicit step.
- **Expanding env vars in ALL config fields:** Only `api_key` needs expansion in this phase. Don't add generic "expand all strings" logic that could have unexpected side effects on URLs, model names, etc. If general expansion is needed later, it can be added to specific fields.
- **Silently falling back when `${VAR}` is missing:** The requirement (ENV-02) says config loading MUST fail with a clear error. Never substitute empty string or skip the variable.
- **Logging the resolved key value:** Even at debug/trace level, never log the actual key. Log the source (literal/env-expanded/convention) and the env var name, but not the value. The `ApiKey` type protects against this, but be careful in the expansion phase before wrapping.
- **Modifying the existing `parse_str` method signature:** Keep `parse_str` for backward compatibility (tests use it). Add a new `from_str_with_env` method that includes expansion. Or update `parse_str` to also expand, but keep it callable from tests.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| `${VAR}` pattern matching | Regex-based expander | Simple `find("${")` loop | No regex dependency needed; pattern is trivial; prior decision says stdlib sufficient |
| Env var lookup | Custom env management | `std::env::var()` | Standard library covers this exactly |
| Secret wrapping | Manual zeroize | `ApiKey(SecretString::from(value))` | Already built in Phase 5 |
| TOML parsing | Custom parser | `toml::from_str::<RawConfig>()` | Already using toml crate |

**Key insight:** The env var expansion logic is intentionally simple -- find `${`, find `}`, look up the name between them. This is not a general-purpose template engine. Keeping it simple means fewer edge cases and clearer error messages.

## Common Pitfalls

### Pitfall 1: Secret leaking during expansion phase
**What goes wrong:** Between TOML parse (raw string) and ApiKey wrapping, the resolved key exists as a plain `String`. If this string is logged or included in error messages, the key leaks.
**Why it happens:** The expansion function operates on `String` values before they become `ApiKey`.
**How to avoid:** Never log the value returned by `expand_env_vars()` or `std::env::var()`. Log only the variable name and whether it resolved. Wrap into `ApiKey` as soon as possible after expansion.
**Warning signs:** Any `tracing::debug!(value = %expanded_value, ...)` call in the expansion code path.

### Pitfall 2: Incorrect provider name to env var mapping
**What goes wrong:** Provider name "my-provider" maps to `ARBSTR_MY_PROVIDER_API_KEY` but user sets `ARBSTR_MYPROVIDER_API_KEY` (no underscore for hyphen).
**Why it happens:** The name-to-env-var transformation isn't intuitive to all users.
**How to avoid:** Document the exact transformation rules. Log the expected env var name in the startup output even when the var is NOT found, so users can see what arbstr is looking for. The `check` command should show the expected convention var name for each provider.
**Warning signs:** Users report "I set the env var but arbstr doesn't see it."

### Pitfall 3: Partial `${` without closing `}`
**What goes wrong:** A config value like `${MISSING_CLOSE` causes either a panic (if using unchecked slice) or confusing behavior.
**Why it happens:** User typo in config file.
**How to avoid:** The expansion function must detect unclosed `${` and return a clear error: `"Unclosed '${' in value"`. Test this case explicitly.
**Warning signs:** Panic on malformed config values.

### Pitfall 4: `${}` with empty variable name
**What goes wrong:** A config value `${}` (empty braces) calls `std::env::var("")` which returns `NotPresent`.
**Why it happens:** User typo or test edge case.
**How to avoid:** Check for empty variable names explicitly and return a descriptive error: `"Empty variable name in '${}'."` This is more helpful than the generic "env var not found" error.
**Warning signs:** Confusing "variable '' not set" error message.

### Pitfall 5: Test interference from real environment variables
**What goes wrong:** Tests that set env vars with `std::env::set_var` interfere with each other when run in parallel.
**Why it happens:** Environment variables are process-global state. Cargo runs tests in parallel by default.
**How to avoid:** Use `std::env::set_var` / `std::env::remove_var` in tests, but use unique variable names per test (e.g., `TEST_EXPAND_01_KEY`). Alternatively, make the expansion function accept a lookup closure `Fn(&str) -> Option<String>` instead of calling `std::env::var` directly. This allows tests to provide a mock environment without touching global state.
**Warning signs:** Tests pass individually but fail when run together with `cargo test`.

### Pitfall 6: Forgetting to update the `check` command
**What goes wrong:** The `check` command validates config syntax but doesn't report env var status, violating ENV-05.
**Why it happens:** The check command currently just calls `Config::from_file` and prints summary info.
**How to avoid:** The `check` command needs to call the env-var-aware loading path and print per-provider key availability and source. It should also report which `${VAR}` references resolve and which don't.
**Warning signs:** `cargo run -- check -c config.toml` doesn't mention env vars at all.

### Pitfall 7: Breaking the existing `parse_str` test interface
**What goes wrong:** Existing tests use `Config::parse_str(toml_str)` which bypasses env var expansion. If `parse_str` is changed to require env var expansion, many tests need env var setup.
**Why it happens:** Tests construct TOML with literal api_key values and expect them to work without env vars.
**How to avoid:** Keep `Config::parse_str` as-is (literal-only, no expansion) for backward compatibility. Add `Config::from_str_with_env` as the new entry point that includes expansion. The `from_file` method calls `from_str_with_env`. Tests can choose which method to use.
**Warning signs:** Existing tests fail because they don't set environment variables.

## Code Examples

### Environment Variable Expansion (complete implementation)

```rust
// Source: hand-rolled pattern per prior decision (no new crates)

/// Expand all `${VAR}` references in a string using environment variables.
///
/// Supports multiple references in one string: `${A}://${B}` expands both.
/// Fails on first missing variable with a clear error.
fn expand_env_vars(input: &str, provider_name: &str) -> Result<String, ConfigError> {
    if !input.contains("${") {
        return Ok(input.to_string());
    }

    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];

        let end = after.find('}').ok_or_else(|| {
            ConfigError::EnvVar {
                var: "<unclosed>".to_string(),
                provider: provider_name.to_string(),
                message: format!("Unclosed '${{' in config value: {}", input),
            }
        })?;

        let var_name = &after[..end];
        if var_name.is_empty() {
            return Err(ConfigError::EnvVar {
                var: "".to_string(),
                provider: provider_name.to_string(),
                message: "Empty variable name in '${}' reference".to_string(),
            });
        }

        let value = std::env::var(var_name).map_err(|_| {
            ConfigError::EnvVar {
                var: var_name.to_string(),
                provider: provider_name.to_string(),
                message: format!(
                    "Environment variable '{}' is not set (referenced in provider '{}')",
                    var_name, provider_name
                ),
            }
        })?;

        result.push_str(&value);
        rest = &after[end + 1..];
    }

    result.push_str(rest);
    Ok(result)
}
```

### Convention-Based Key Lookup

```rust
// Source: custom pattern for ARBSTR_<NAME>_API_KEY convention

/// Derive the convention env var name for a provider.
fn convention_env_var_name(provider_name: &str) -> String {
    format!(
        "ARBSTR_{}_API_KEY",
        provider_name.to_uppercase().replace('-', "_").replace(' ', "_")
    )
}

/// Try convention-based env var lookup for a provider's API key.
fn convention_key_lookup(provider_name: &str) -> Option<(String, String)> {
    let var_name = convention_env_var_name(provider_name);
    std::env::var(&var_name).ok().map(|value| (var_name, value))
}
```

### Key Source Enum

```rust
/// How a provider's API key was resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum KeySource {
    /// Key was a literal string in config (no ${} references)
    Literal,
    /// Key contained ${VAR} references expanded from environment
    EnvExpanded,
    /// Key was auto-discovered from convention env var (holds var name)
    Convention(String),
    /// No key available
    None,
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Literal => write!(f, "config-literal"),
            KeySource::EnvExpanded => write!(f, "env-expanded"),
            KeySource::Convention(var) => write!(f, "convention ({})", var),
            KeySource::None => write!(f, "none"),
        }
    }
}
```

### New Error Variant

```rust
// Added to ConfigError enum in src/config.rs

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    // ... existing variants ...

    #[error("Environment variable '{var}' not set for provider '{provider}': {message}")]
    EnvVar {
        var: String,
        provider: String,
        message: String,
    },
}
```

### RawConfig to Config Conversion

```rust
impl Config {
    /// Convert raw (deserialized) config to final config with env var expansion.
    fn from_raw(raw: RawConfig) -> Result<(Self, Vec<(String, KeySource)>), ConfigError> {
        let mut providers = Vec::with_capacity(raw.providers.len());
        let mut key_sources = Vec::with_capacity(raw.providers.len());

        for rp in raw.providers {
            let (api_key, source) = match rp.api_key {
                Some(ref raw_key) if raw_key.contains("${") => {
                    // Phase 2: Expand env var references
                    let expanded = expand_env_vars(raw_key, &rp.name)?;
                    (Some(ApiKey::from(expanded)), KeySource::EnvExpanded)
                }
                Some(raw_key) => {
                    // Literal key in config
                    (Some(ApiKey::from(raw_key)), KeySource::Literal)
                }
                None => {
                    // Phase 3: Convention-based lookup
                    match convention_key_lookup(&rp.name) {
                        Some((var_name, value)) => {
                            (Some(ApiKey::from(value)), KeySource::Convention(var_name))
                        }
                        None => (None, KeySource::None),
                    }
                }
            };

            key_sources.push((rp.name.clone(), source));

            providers.push(ProviderConfig {
                name: rp.name,
                url: rp.url,
                api_key,
                models: rp.models,
                input_rate: rp.input_rate,
                output_rate: rp.output_rate,
                base_fee: rp.base_fee,
            });
        }

        let config = Config {
            server: raw.server,
            database: raw.database,
            providers,
            policies: raw.policies,
            logging: raw.logging,
        };

        Ok((config, key_sources))
    }
}
```

### Updated `check` Command (ENV-05)

```rust
Commands::Check { config: config_path } => {
    // Use the env-var-aware loading path
    let content = std::fs::read_to_string(&config_path)?;
    let raw: RawConfig = toml::from_str(&content)?;
    let (config, key_sources) = Config::from_raw(raw)?;
    config.validate()?;

    println!("Configuration is valid!");
    println!("  Listen: {}", config.server.listen);
    println!("  Providers: {}", config.providers.len());
    println!("  Policy rules: {}", config.policies.rules.len());
    println!();
    println!("Provider key status:");
    for (name, source) in &key_sources {
        println!("  {}: {}", name, source);
    }
    Ok(())
}
```

### Testable Expansion with Lookup Closure

```rust
/// Expand env vars using a custom lookup function (testable without global env).
fn expand_env_vars_with<F>(input: &str, provider_name: &str, lookup: F) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    // Same logic as expand_env_vars but uses lookup(var_name) instead of std::env::var(var_name)
    // ...
}

/// Convenience wrapper that uses std::env::var.
fn expand_env_vars(input: &str, provider_name: &str) -> Result<String, ConfigError> {
    expand_env_vars_with(input, provider_name, |name| std::env::var(name).ok())
}
```

### Test: Env var expansion

```rust
#[test]
fn test_expand_single_var() {
    let lookup = |name: &str| match name {
        "MY_KEY" => Some("cashuABCD".to_string()),
        _ => None,
    };
    let result = expand_env_vars_with("${MY_KEY}", "test", lookup).unwrap();
    assert_eq!(result, "cashuABCD");
}

#[test]
fn test_expand_missing_var_fails() {
    let lookup = |_: &str| None;
    let result = expand_env_vars_with("${MISSING}", "provider-alpha", lookup);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("MISSING"), "Error should name the variable");
    assert!(err.contains("provider-alpha"), "Error should name the provider");
}

#[test]
fn test_expand_multiple_vars() {
    let lookup = |name: &str| match name {
        "SCHEME" => Some("https".to_string()),
        "HOST" => Some("example.com".to_string()),
        _ => None,
    };
    let result = expand_env_vars_with("${SCHEME}://${HOST}/v1", "test", lookup).unwrap();
    assert_eq!(result, "https://example.com/v1");
}

#[test]
fn test_expand_no_vars_passthrough() {
    let lookup = |_: &str| -> Option<String> { panic!("should not be called") };
    let result = expand_env_vars_with("literal-value", "test", lookup).unwrap();
    assert_eq!(result, "literal-value");
}

#[test]
fn test_convention_env_var_name() {
    assert_eq!(convention_env_var_name("alpha"), "ARBSTR_ALPHA_API_KEY");
    assert_eq!(convention_env_var_name("provider-beta"), "ARBSTR_PROVIDER_BETA_API_KEY");
    assert_eq!(convention_env_var_name("my_service"), "ARBSTR_MY_SERVICE_API_KEY");
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `config` crate's `Environment` source | Direct TOML + custom expansion | Always valid for small projects | config crate was already removed in Phase 5; hand-rolled expansion gives precise control over syntax and error messages |
| Generic string interpolation (all fields) | Targeted expansion (api_key only) | Design choice | Prevents unexpected behavior in URL, model name, or other fields |

**Deprecated/outdated:**
- The `config` crate (removed in Phase 5) provided generic env var layering but was never used. The current approach of TOML + targeted expansion is simpler and more explicit.
- Shell-style `$VAR` (without braces) is intentionally NOT supported. Only `${VAR}` (with braces) is recognized. This avoids ambiguity with literal dollar signs in values and is consistent with Docker Compose, GitHub Actions, and other modern config formats.

## Open Questions

1. **Should `${VAR}` expansion apply to fields beyond `api_key`?**
   - What we know: The requirements (ENV-01 through ENV-05) focus exclusively on API keys. The success criteria mention only `api_key`.
   - What's unclear: Whether `url` or other fields should also support `${VAR}` expansion.
   - Recommendation: **Expand only `api_key` in this phase.** The requirements are clear. If URL expansion is needed later, the `expand_env_vars` function can be reused. Expanding all fields introduces risk (what if a model name contains `${`?).

2. **Should the expansion function be a closure-based abstraction from the start?**
   - What we know: A closure-based `expand_env_vars_with(input, provider, lookup_fn)` makes unit testing trivial (no global env state). A direct `expand_env_vars(input, provider)` calling `std::env::var` is simpler but harder to test.
   - What's unclear: Whether the test complexity justifies the abstraction.
   - Recommendation: **Use the closure-based abstraction.** It adds one extra parameter but eliminates test flakiness from global env state. The convenience wrapper `expand_env_vars(input, provider)` calls it with `std::env::var`. Tests use the `_with` variant directly.

3. **Should `from_raw` return key sources alongside Config, or should they be stored in Config?**
   - What we know: Key sources are needed at startup (logging) and by the `check` command. They are NOT needed at request time.
   - What's unclear: Whether to store them in Config or return them as a side value.
   - Recommendation: **Return as a side value `Vec<(String, KeySource)>`.** Key sources are ephemeral startup metadata, not runtime configuration. Storing them in Config would pollute the runtime struct with data never used after startup.

## Sources

### Primary (HIGH confidence)
- [std::env::var documentation](https://doc.rust-lang.org/std/env/fn.var.html) - API signature, VarError::NotPresent and VarError::NotUnicode return types
- [shellexpand docs.rs](https://docs.rs/shellexpand/latest/shellexpand/) - API reference for the leading env expansion crate (not used, but evaluated as alternative)
- [envsubst docs.rs](https://docs.rs/envsubst) - API reference for envsubst crate (not used, evaluated as alternative)

### Codebase (HIGH confidence)
- `src/config.rs` - Current Config, ProviderConfig, ApiKey, ConfigError, parse_str, from_file, validate methods
- `src/main.rs` - Current check/serve/providers command implementations, mock_config()
- `src/error.rs` - Current Error enum with Config variant
- `src/proxy/handlers.rs` - Uses `provider.api_key.expose_secret()` for Authorization header
- `src/router/selector.rs` - Uses `ProviderConfig.api_key` via SelectedProvider
- `Cargo.toml` - Current dependencies (no regex, secrecy 0.10 present)

### Prior Decisions (HIGH confidence)
- Phase 5 Research: "Two-phase config loading (Raw -> expand -> SecretString) for clean env var integration"
- Phase 5 Research: "No new crates for env expansion -- stdlib std::env::var is sufficient"
- Phase 5 Research: "Remove unused config crate dependency" (already done)
- Phase 5-01: "Custom Deserialize impl uses String then wraps, avoiding SecretString serde complexity" -- confirmed in codebase: `String::deserialize(d).map(|s| ApiKey(SecretString::from(s)))`

### Secondary (MEDIUM confidence)
- [shellexpand on lib.rs](https://lib.rs/crates/shellexpand) - Version 3.1.1, 2M downloads/week, last updated April 2025 -- confirms it's the ecosystem standard for shell-like expansion but overkill for this use case
- [Docker Compose env var syntax](https://docs.docker.com/compose/) - Industry precedent for `${VAR}` syntax in config files
- [GitHub Actions env var syntax](https://docs.github.com/en/actions) - Industry precedent for `${VAR}` syntax

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - stdlib only, no new dependencies, prior decision confirmed
- Architecture: HIGH - two-phase loading pattern established in Phase 5 research, implementation details verified against current codebase
- Pitfalls: HIGH - based on direct code analysis and known Rust testing patterns (global env state, string slicing safety)

**Research date:** 2026-02-15
**Valid until:** 90 days (stdlib APIs are stable; architecture patterns are project-specific and won't change externally)
