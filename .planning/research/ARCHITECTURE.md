# Architecture Patterns: Secrets Handling

**Domain:** Env var expansion and secret redaction for Rust/serde/tracing LLM proxy
**Researched:** 2026-02-15
**Overall confidence:** HIGH (secrecy crate well-documented, serde patterns well-established, verified against current codebase)

## Current Architecture (Baseline)

### How API Keys Flow Today

```
config.toml                    ProviderConfig              SelectedProvider           HTTP Request
+-----------------+    serde   +------------------+  clone  +------------------+  inject  +-------------+
| api_key = "sk-" | ---------> | api_key: Option   | ------> | api_key: Option   | -------> | Bearer sk-  |
|                 | Deserialize|        <String>   |  From   |        <String>   |  header  |             |
+-----------------+            +------------------+         +------------------+          +-------------+
```

### Leak Surfaces (Current Code)

| Surface | File | Line(s) | Mechanism | Risk |
|---------|------|---------|-----------|------|
| Debug derive on ProviderConfig | config.rs | 52 | `#[derive(Debug)]` prints all fields including `api_key` | Any `{:?}` formatting leaks key |
| Debug derive on SelectedProvider | selector.rs | 9 | `#[derive(Debug)]` prints `api_key` | Same as above |
| Debug derive on Config | config.rs | 7 | `#[derive(Debug)]` on Config (contains Vec\<ProviderConfig\>) | Entire config debug output leaks all keys |
| /providers endpoint | handlers.rs | 764-784 | Manual `serde_json::json!` -- currently safe (excludes api_key) | Safe now but fragile; adding Serialize derive would leak |
| tracing spans with provider info | handlers.rs | 576-580, 504 | `tracing::info!(provider = ...)` | Provider name logged, not key -- safe but close |
| Error messages | handlers.rs | 506-508 | `format!("Failed to reach provider '{}': {}", provider.name, e)` | Safe -- only name, not key |
| Clone proliferation | selector.rs | 24 | `config.api_key.clone()` as plain String | Each clone is another copy of plaintext in memory |

**Assessment:** The /providers endpoint manually constructs JSON without api_key, so it is safe. The primary leak risk is Debug formatting -- any `dbg!()`, `{:?}` log, or panic that includes ProviderConfig or SelectedProvider prints API keys in plaintext.

### How Config Parsing Works Today

```rust
// config.rs:137-150
pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
    let content = std::fs::read_to_string(path.as_ref())?;
    Self::parse_str(&content)
}

pub fn parse_str(content: &str) -> Result<Self, ConfigError> {
    let config: Config = toml::from_str(content).map_err(ConfigError::Parse)?;
    config.validate()?;
    Ok(config)
}
```

Pipeline: read file -> TOML string -> serde Deserialize -> validate -> done. No env var expansion step. No wrapping of sensitive fields.

## Recommended Architecture

### Design Decision: Two-Phase Config Loading

There are two viable approaches for integrating env var expansion and SecretString wrapping:

**Approach A -- `deserialize_with` (single-phase):** Expansion and wrapping happen inside serde via a custom `deserialize_with` function on the api_key field. Pro: no struct duplication. Con: error messages from serde lose field context (serde reports generic "expected a string" without identifying which provider's api_key failed).

**Approach B -- Two-phase with RawConfig (recommended):** Deserialize into a `RawProviderConfig` with plain `Option<String>`, then expand env vars and convert to `ProviderConfig` with `Option<SecretString>` in a separate step. Pro: better error messages with provider name context, cleaner separation of concerns (parsing vs. env resolution), convention-based fallback is natural to add. Con: duplicates the struct definition.

**Recommendation: Approach B (two-phase).** The struct duplication is minor (RawProviderConfig is internal/non-pub, ~15 lines). The error message quality matters for user experience -- when `ROUTSTR_KEY` is not set, the error should say which provider is affected. Convention-based env var lookup (ARBSTR_PROVIDER_ALPHA_API_KEY) is straightforward to add in the conversion step but awkward inside a serde deserializer.

### Component Diagram

```
config.toml                     Phase 1: serde                   Phase 2: resolve
+-----------------------+    +----------------------+    +---------------------------+
| api_key = "${API_KEY}" | -->| RawProviderConfig    |--->| expand_env_vars()          |
|                         |    |   api_key: Option     |    |   "${API_KEY}" -> "cashu.." |
|                         |    |          <String>     |    | convention_fallback()      |
+-----------------------+    +----------------------+    |   check ARBSTR_<NAME>_KEY  |
                                                           | wrap in SecretString       |
                                                           +---------------------------+
                                                                      |
                                                                      v
                                                              ProviderConfig
                                                              +----------------------+
                                                              | api_key: Option<      |
                                                              |   SecretString>       |
                                                              | Debug: "[REDACTED]"   |
                                                              +----------------------+
                                                                      |
                                                      From<&ProviderConfig>
                                                                      v
                                                              SelectedProvider
                                                              +----------------------+
                                                              | api_key: Option<      |
                                                              |   SecretString>       |
                                                              +----------------------+
                                                                      |
                                                          expose_secret() (1 call site)
                                                                      v
                                                              handlers.rs
                                                              +----------------------+
                                                              | Bearer {exposed_val}  |
                                                              +----------------------+
```

### New Components

| Component | Location | Type | Purpose |
|-----------|----------|------|---------|
| `RawProviderConfig` | `src/config.rs` (internal) | struct, `#[derive(Deserialize)]` | TOML deserialization target with plain String fields |
| `expand_env_vars` | `src/secret.rs` (new module) | `fn(&str) -> Result<String, String>` | Pure function: `"${VAR}"` -> env var value, literal passthrough |
| `convention_env_key` | `src/secret.rs` (new module) | `fn(&str) -> String` | Derives `ARBSTR_<NAME>_API_KEY` from provider name |
| SecretString re-export | `src/secret.rs` (new module) | re-export from `secrecy` | Single import point for the rest of the codebase |

### Modified Components

| Component | File | Change |
|-----------|------|--------|
| `ProviderConfig` | config.rs | Remove `#[derive(Deserialize)]`, add `api_key: Option<SecretString>`, keep `#[derive(Debug, Clone)]` |
| `Config::parse_str` | config.rs | Deserialize into RawConfig, then convert via `into_config()` |
| `SelectedProvider.api_key` | selector.rs:13 | `Option<String>` -> `Option<SecretString>` |
| `From<&ProviderConfig> for SelectedProvider` | selector.rs:19-27 | `.clone()` on SecretString (still works -- SecretString: Clone) |
| `send_to_provider` | handlers.rs:498-501 | `api_key.expose_secret()` for Bearer header |
| Mock providers in main.rs | main.rs:155,168 | `api_key: None` stays `None` -- no change |
| Tests in selector.rs | selector.rs:229+ | `api_key: None` stays `None` -- no change |

### Unchanged Components

| Component | Why Unchanged |
|-----------|--------------|
| `/providers` endpoint (handlers.rs:764-784) | Already manually constructs JSON without api_key field |
| Error types (error.rs) | Never contain api_key |
| Tracing spans (server.rs, handlers.rs) | Log provider name/URL, not api_key |
| PoliciesConfig, ServerConfig, DatabaseConfig, LoggingConfig | No secret fields |
| Retry logic (retry.rs) | CandidateInfo carries name/url, not api_key |

## Pattern Details

### Pattern 1: SecretString via secrecy Crate

**What:** Use `secrecy::SecretString` (which is `SecretBox<String>`) to wrap API keys. Provides automatic Debug redaction (`Secret([REDACTED])`), explicit access via `expose_secret()`, and memory zeroing on drop via `zeroize`.

**Why secrecy:**
- Ecosystem standard (iqlusioninc, used by major Rust projects)
- Version 0.10.3 is current and well-maintained
- Has a `serde` feature flag for Deserialize support (useful for tests, not used in two-phase approach)
- Prevents accidental Serialize by default (no Serialize impl)
- String already implements Zeroize, so SecretString works with no extra trait impls
- The `redact` crate was evaluated but rejected -- secrecy is more widely adopted and includes zeroize by default

**Cargo.toml addition:**
```toml
secrecy = { version = "0.10", features = ["serde"] }
```

**Confidence:** HIGH -- secrecy crate docs confirm Debug output is `Secret([REDACTED])` and `expose_secret()` returns `&T`.

### Pattern 2: Two-Phase Config Deserialization

**What:** Deserialize TOML into `RawProviderConfig` (plain String), then convert to `ProviderConfig` (SecretString) after env var expansion.

**Implementation sketch:**
```rust
// Internal -- not exported
#[derive(Deserialize)]
struct RawProviderConfig {
    pub name: String,
    pub url: String,
    pub api_key: Option<String>,  // raw, may contain ${VAR}
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub input_rate: u64,
    #[serde(default)]
    pub output_rate: u64,
    #[serde(default)]
    pub base_fee: u64,
}

// Public config with secrets wrapped
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub name: String,
    pub url: String,
    pub api_key: Option<SecretString>,
    pub models: Vec<String>,
    pub input_rate: u64,
    pub output_rate: u64,
    pub base_fee: u64,
}
```

**Conversion with env var expansion:**
```rust
impl RawProviderConfig {
    fn into_provider_config(self) -> Result<ProviderConfig, ConfigError> {
        // Phase 1: Expand ${VAR} if present
        let api_key = match self.api_key {
            Some(raw) => {
                let expanded = expand_env_vars(&raw).map_err(|e| {
                    ConfigError::Validation(format!(
                        "Provider '{}': {}", self.name, e
                    ))
                })?;
                Some(SecretString::from(expanded))
            }
            None => {
                // Phase 2: Convention-based fallback
                let env_name = convention_env_key(&self.name);
                std::env::var(&env_name).ok().map(SecretString::from)
            }
        };

        Ok(ProviderConfig {
            name: self.name,
            url: self.url,
            api_key,
            models: self.models,
            input_rate: self.input_rate,
            output_rate: self.output_rate,
            base_fee: self.base_fee,
        })
    }
}
```

**Why two-phase over `deserialize_with`:**
- Error messages include provider name context: `Provider 'alpha': Environment variable 'KEY' not set`
- Convention-based fallback (check ARBSTR_ALPHA_API_KEY if no explicit key) is natural in conversion step
- No mixing of data format parsing with environment state resolution
- RawProviderConfig is internal/non-pub, so the duplication is contained

**Confidence:** HIGH -- standard Rust pattern (intermediate deserialization structs).

### Pattern 3: Env Var Expansion (Pure Function)

**What:** A standalone function that detects `${VAR_NAME}` or `$VAR_NAME` patterns and expands them.

```rust
/// Expand environment variable references in a string value.
/// Supports `${VAR_NAME}` (braced) and `$VAR_NAME` (simple) syntax.
/// Returns the original string if no env var pattern is detected.
pub fn expand_env_vars(input: &str) -> Result<String, String> {
    // ${VAR_NAME} -- braced syntax
    if let Some(var_name) = input.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        if var_name.is_empty() {
            return Err("Empty variable name in ${}".to_string());
        }
        return std::env::var(var_name)
            .map_err(|_| format!("Environment variable '{}' not set", var_name));
    }
    // $VAR_NAME -- simple syntax (entire value must be the reference)
    if let Some(var_name) = input.strip_prefix('$') {
        if !var_name.is_empty()
            && var_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return std::env::var(var_name)
                .map_err(|_| format!("Environment variable '{}' not set", var_name));
        }
    }
    // No pattern -- return as-is (literal value)
    Ok(input.to_string())
}
```

**Design choices:**
- Entire value must be the env var reference (no embedded `"prefix-${VAR}-suffix"` -- avoids partial expansion complexity)
- Fail on missing var rather than silent empty string (fail-fast at startup)
- Pure function, trivially unit-testable

**Confidence:** HIGH -- stdlib `std::env::var`, simple string matching.

### Pattern 4: Convention-Based Env Var Fallback

**What:** If no `api_key` is set in TOML, check `ARBSTR_<NORMALIZED_NAME>_API_KEY` as a fallback.

```rust
/// Derive a convention-based env var name from a provider name.
/// "provider-alpha" -> "ARBSTR_PROVIDER_ALPHA_API_KEY"
pub fn convention_env_key(provider_name: &str) -> String {
    let normalized = provider_name
        .to_ascii_uppercase()
        .replace(['-', '.', ' '], "_");
    format!("ARBSTR_{}_API_KEY", normalized)
}
```

**Precedence (highest wins):**
1. Explicit `api_key` in TOML with `${VAR}` expanded
2. Explicit `api_key` in TOML as literal value
3. Convention-based `ARBSTR_<NAME>_API_KEY` env var
4. No key (provider used without authentication)

**Why:** Follows Docker/12-factor conventions. Users managing many providers can set env vars without editing config. Matches how tools like docker-compose handle secrets.

**Confidence:** HIGH -- trivial string manipulation, industry-standard convention.

### Pattern 5: Type-Driven Redaction

**What:** By changing the type from `String` to `SecretString`, the compiler enforces redaction at every Debug call site automatically.

**Surfaces automatically protected by this change:**

| Surface | Before | After |
|---------|--------|-------|
| `println!("{:?}", config)` | Prints plaintext key | Prints `Secret([REDACTED])` |
| `dbg!(provider)` | Prints plaintext key | Prints `Secret([REDACTED])` |
| `tracing::debug!(?config)` | Prints plaintext key | Prints `Secret([REDACTED])` |
| Panic backtrace with provider | Prints plaintext key | Prints `Secret([REDACTED])` |

**Surfaces requiring manual discipline:**

| Surface | Risk | Mitigation |
|---------|------|------------|
| `expose_secret()` result logged | Developer exposes then logs | Code review: grep for `expose_secret` -- should appear in exactly 1 production call site |
| Serialize derive added to ProviderConfig | Would fail to compile | SecretString has no Serialize impl -- compiler prevents this |
| New secret field added as plain String | Not wrapped | Document convention in CLAUDE.md |

SecretString also does not implement `Display`, so `tracing::info!(key = %api_key)` would fail at compile time. Only `?` (Debug) works, which outputs `[REDACTED]`. This is a two-layer defense: Display prevents use in format strings, Debug redacts when used in debug output.

**Confidence:** HIGH -- secrecy's Debug impl is well-documented; absence of Display is confirmed in crate source.

### Pattern 6: Minimize expose_secret() Call Sites

**What:** Restrict `expose_secret()` to exactly 1 call site in production code.

**Target call site:**
```rust
// handlers.rs: send_to_provider()
if let Some(api_key) = &provider.api_key {
    upstream_request = upstream_request.header(
        header::AUTHORIZATION,
        format!("Bearer {}", api_key.expose_secret()),
    );
}
```

**Monitoring:** `grep -rn "expose_secret" src/` should return exactly 1 result in production code (plus test code if tests construct SecretStrings).

**Confidence:** HIGH -- straightforward code organization principle.

## Anti-Patterns to Avoid

### Anti-Pattern 1: String-Based Redaction at Log Sites

**What:** Using helper functions like `redact_key(key)` at every tracing call site.

**Why bad:** One forgotten call site leaks the secret. Does not protect Debug derive output. Does not prevent serde serialization. Relies on discipline rather than compiler enforcement.

**Instead:** Use SecretString. The compiler enforces redaction.

### Anti-Pattern 2: Hand-Rolling a Secret Wrapper

**What:** Creating a custom `struct Secret(String)` with manual Debug/Clone/Zeroize.

**Why bad:** Getting zeroize right is subtle (must override Drop, must prevent compiler from optimizing away zeroing, must handle reallocation leaving old copies). The secrecy crate is audited for these edge cases.

**Instead:** Use `secrecy::SecretString`. Pulls in only `zeroize` as a transitive dependency.

### Anti-Pattern 3: Expanding All String Fields

**What:** Running env var expansion on every string field in the config.

**Why bad:** `listen = "${PORT}"` looks like it should work but listen expects "host:port" format. `name = "${PROVIDER_NAME}"` is confusing -- provider names should be static identifiers. Creates ambiguity about where `${...}` syntax is supported.

**Instead:** Only expand `api_key` (and potentially `url` if users request it). Document which fields support expansion.

### Anti-Pattern 4: Exposing Secrets in Error Messages

**What:** Including full provider context in client-facing errors.

**Why bad:** reqwest errors may include the full URL with query params. Provider error bodies may echo back credentials: `"Invalid API key: cashuA1234..."`. These errors flow to HTTP clients via `IntoResponse`.

**Instead:** Separate internal detail (logged server-side via tracing) from external message (returned to client). The current code already does this well for provider names; extend the discipline to any error path that might contain credential data.

### Anti-Pattern 5: Adding Serialize to Config Structs

**What:** Adding `#[derive(Serialize)]` to ProviderConfig.

**Why bad:** SecretString has no Serialize impl by default. Adding Serialize to the struct causes a compiler error, which is the intended safety behavior. Working around it (e.g., `#[serde(skip)]` on api_key) partially defeats the purpose.

**Instead:** For config export, manually construct JSON (as /providers endpoint already does) excluding secret fields.

## Data Flow (After Changes)

### Happy Path: Env Var Reference

```
1. config.toml: api_key = "${ROUTSTR_API_KEY}"

2. Config::parse_str() -> toml::from_str() into RawConfig
   -> RawProviderConfig { api_key: Some("${ROUTSTR_API_KEY}"), ... }

3. into_provider_config():
   -> expand_env_vars("${ROUTSTR_API_KEY}")
   -> std::env::var("ROUTSTR_API_KEY") -> "cashuA..."
   -> SecretString::from("cashuA...")
   -> ProviderConfig { api_key: Some(SecretString), ... }

4. ProviderRouter stores Vec<ProviderConfig>

5. Request: router selects -> SelectedProvider via From<&ProviderConfig>
   -> api_key.clone() (SecretString clone, still redacted)

6. send_to_provider():
   -> api_key.expose_secret() -> &str -> Bearer header
```

### Error Path: Missing Env Var

```
1. config.toml: api_key = "${NONEXISTENT_VAR}"

2. toml::from_str() succeeds:
   -> RawProviderConfig { api_key: Some("${NONEXISTENT_VAR}"), ... }

3. into_provider_config():
   -> expand_env_vars("${NONEXISTENT_VAR}")
   -> std::env::var fails
   -> Err("Environment variable 'NONEXISTENT_VAR' not set")
   -> ConfigError::Validation("Provider 'alpha': Environment variable 'NONEXISTENT_VAR' not set")

4. Startup fails with actionable error message including provider name
```

### Happy Path: Convention-Based Fallback

```
1. config.toml: no api_key field for provider "relay-one"
   Environment: ARBSTR_RELAY_ONE_API_KEY=cashuB...

2. toml::from_str():
   -> RawProviderConfig { api_key: None, name: "relay-one", ... }

3. into_provider_config():
   -> api_key is None, check convention
   -> convention_env_key("relay-one") -> "ARBSTR_RELAY_ONE_API_KEY"
   -> std::env::var("ARBSTR_RELAY_ONE_API_KEY") -> "cashuB..."
   -> SecretString::from("cashuB...")
   -> ProviderConfig { api_key: Some(SecretString), ... }
```

### Happy Path: Literal Key

```
1. config.toml: api_key = "cashuA1234..."

2. expand_env_vars("cashuA1234..."):
   -> No ${} or $ pattern match
   -> Ok("cashuA1234...") as-is
   -> SecretString::from("cashuA1234...")

3. Identical behavior to today, but now redacted in Debug output
```

## Build Order (Dependency Graph)

```
Step 1: Cargo.toml
   |     Add: secrecy = { version = "0.10", features = ["serde"] }
   |
   v
Step 2: src/secret.rs (new module) + src/lib.rs
   |     - pub mod secret in lib.rs
   |     - Re-export SecretString and ExposeSecret from secrecy
   |     - expand_env_vars() pure function
   |     - convention_env_key() pure function
   |     - Unit tests for both functions
   |     (Fully testable in isolation, no other code changes yet)
   |
   v
Step 3: src/config.rs -- THE BREAKING CHANGE
   |     - Add internal RawConfig, RawProviderConfig (Deserialize)
   |     - Change ProviderConfig: remove Deserialize, add api_key: Option<SecretString>
   |     - Add into_config() conversion with expansion + convention fallback
   |     - Update parse_str(): deserialize into RawConfig, then into_config()
   |     - Update config tests
   |     NOTE: Compiler errors now appear in selector.rs, handlers.rs, main.rs
   |
   v
Step 4: src/router/selector.rs
   |     - SelectedProvider.api_key: Option<String> -> Option<SecretString>
   |     - From impl: .clone() works unchanged
   |     - Test fixtures: api_key: None stays None
   |
   v
Step 5: src/proxy/handlers.rs
   |     - Add: use secrecy::ExposeSecret;
   |     - send_to_provider: api_key.expose_secret() for Bearer header
   |     - This is the ONLY expose_secret() call site
   |
   v
Step 6: src/main.rs (mock providers)
         - api_key: None stays None
         - Verify mock mode compiles and runs
```

**Why this order:**
- Steps 1-2 are additive: new dependency and new module, zero existing code changes, fully testable in isolation
- Step 3 is the breaking change: after this, the compiler tells you exactly which files need updating via type mismatch errors
- Steps 4-6 fix compiler errors in dependency order (config -> router -> handlers)
- The compiler-driven approach makes it impossible to miss a call site

## Scalability Considerations

Not a concern for this milestone. Secret handling is startup-time work with negligible runtime cost:

| Concern | Cost |
|---------|------|
| Env var expansion at startup | Microseconds (one `std::env::var` per provider) |
| Convention key derivation | Negligible string manipulation |
| SecretString vs String runtime | Single pointer indirection for `expose_secret()` |
| Zeroize on drop | One memset when process exits or config reloads |
| Clone cost | Identical to String clone |

## Sources

- [secrecy crate on crates.io](https://crates.io/crates/secrecy) -- version 0.10.3, latest stable
- [secrecy API docs](https://docs.rs/secrecy/latest/secrecy/) -- SecretString, ExposeSecret, Debug behavior, serde feature
- [secrecy GitHub (iqlusioninc/crates)](https://github.com/iqlusioninc/crates/tree/main/secrecy) -- source and README
- [Secure Configuration and Secrets Management in Rust](https://leapcell.io/blog/secure-configuration-and-secrets-management-in-rust-with-secrecy-and-environment-variables) -- practical integration patterns
- [redact crate](https://crates.io/crates/redact) -- evaluated alternative, rejected (secrecy more widely adopted, includes zeroize)
- [serde-with-expand-env](https://github.com/Roger/serde-with-expand-env) -- evaluated, rejected (custom ~30 lines is sufficient)
- [serde-env-field](https://lib.rs/crates/serde-env-field) -- evaluated, rejected (over-engineering for one field)
- [Docker Compose variable interpolation](https://docs.docker.com/compose/how-tos/environment-variables/variable-interpolation/) -- industry convention for `${VAR}` syntax
- [Rust users forum: secrecy + Serialize](https://users.rust-lang.org/t/secrecy-crate-serialize-string/112263) -- confirms no Serialize by default is intentional
- Direct codebase analysis of arbstr src/ (config.rs, selector.rs, handlers.rs, main.rs, retry.rs, server.rs, error.rs)
