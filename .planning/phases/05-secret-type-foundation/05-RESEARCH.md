# Phase 5: Secret Type Foundation - Research

**Researched:** 2026-02-15
**Domain:** Rust type-system secret protection (secrecy crate, serde integration, Debug/Serialize redaction)
**Confidence:** HIGH

## Summary

This phase migrates the `api_key` field from `Option<String>` to `Option<SecretString>` (a newtype wrapping `secrecy::SecretString`) across the config, router, and proxy layers. The secrecy crate v0.10.3 provides `SecretBox<str>` (aliased as `SecretString`) with automatic zeroize-on-drop and a Debug impl that prints `SecretBox<str>([REDACTED])`. Since the user requirement is for Debug to emit plain `[REDACTED]` (not `SecretBox<str>([REDACTED])`), a thin newtype wrapper is recommended.

The change is surgical: only the type changes, serde deserialization continues to work transparently with TOML (secrecy provides a dedicated `Deserialize` impl for `SecretString`), and the actual secret value is only ever accessed via `.expose_secret()` which makes every usage point explicit and grep-auditable. The `/providers` endpoint adds `"api_key": "[REDACTED]"` via custom JSON serialization, while the CLI `providers` command drops the key column entirely.

**Primary recommendation:** Create a newtype `ApiKey(secrecy::SecretString)` with custom Debug (`[REDACTED]`), custom Serialize (always `"[REDACTED]"`), delegated Deserialize (via secrecy's built-in serde impl), and an `expose_secret()` method. Use `Option<ApiKey>` everywhere `Option<String>` currently holds api_key.

<user_constraints>

## User Constraints (from CONTEXT.md)

### Locked Decisions

#### Redaction format
- Full `[REDACTED]` replacement everywhere -- no partial masks or prefixes (Phase 7 adds masked prefixes later)
- Same `[REDACTED]` string across all contexts: CLI output, JSON responses, debug/tracing logs
- SecretString Debug impl emits just `[REDACTED]` -- surrounding struct's derive(Debug) handles field names
- Error messages reference provider name only, never any key-related info

#### API key optionality
- api_key stays **required** in this phase -- only the type changes from String to SecretString
- Phase 6 will make it optional for convention-based env var lookup
- TOML config stays exactly the same: `api_key = "cashuA..."` -- serde deserializes transparently into SecretString
- Actual key value accessed via `.expose_secret()` -- makes every access point explicit and grep-auditable
- Mock providers use SecretString too -- consistent type system, tests verify redaction works

#### JSON serialization
- `/providers` endpoint includes `"api_key": "[REDACTED]"` -- field present but redacted, confirms key is configured
- No extra `has_key` boolean -- redacted value presence is sufficient
- CLI `providers` command removes the key column entirely -- a column of identical `[REDACTED]` adds no value
- Custom Serialize impl on the config type to always produce `[REDACTED]` -- do not rely on secrecy's default Serialize which exposes the actual secret

### Claude's Discretion
- Wrapper type design (newtype around secrecy::SecretString vs direct use)
- How to structure the custom Serialize to avoid accidental exposure
- Test strategy for verifying redaction in each output surface

### Deferred Ideas (OUT OF SCOPE)
None -- discussion stayed within phase scope.

</user_constraints>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| secrecy | 0.10.3 | SecretString with zeroize-on-drop | Ecosystem standard for Rust secret management; `no_std`-friendly, `forbid(unsafe_code)`, integrates with serde |
| zeroize | 1.6+ | Secure memory clearing on drop | Transitive dependency of secrecy; compiler-intrinsic memory zeroing |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| serde | 1.x (already in project) | Serialize/Deserialize derives | Existing dependency, secrecy's `serde` feature hooks into it |

### Removable
| Library | Current | Action | Why |
|---------|---------|--------|-----|
| config | 0.14 | Remove from Cargo.toml | Listed but never imported anywhere in source code; dead dependency |

**Installation:**
```toml
# Add to [dependencies] in Cargo.toml
secrecy = { version = "0.10", features = ["serde"] }

# Remove unused dependency
# config = "0.14"  <-- DELETE THIS LINE
```

## Architecture Patterns

### Recommended Wrapper Type: `ApiKey` Newtype

**Rationale for newtype vs direct use of `secrecy::SecretString`:**

The user decision specifies Debug output as just `[REDACTED]`, but secrecy's built-in Debug for SecretBox prints `SecretBox<str>([REDACTED])`. Additionally, secrecy intentionally does NOT implement Serialize for SecretString (String does not implement `SerializableSecret`), and even if it did, it would expose the actual secret. A newtype wrapper solves both issues cleanly.

**Location:** `src/config.rs` (co-located with ProviderConfig where it's used)

```rust
// Source: verified from docs.rs/secrecy/0.10.3

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize, Serializer, Deserializer};

/// API key wrapper that redacts in Debug/Display/Serialize and zeroizes on drop.
///
/// The inner `SecretString` ensures the key value is:
/// - Zeroized in memory when dropped (SEC-02)
/// - Never exposed via Debug or Display (SEC-01)
/// - Only accessible via `.expose_secret()` (grep-auditable)
#[derive(Clone)]
pub struct ApiKey(SecretString);

impl ApiKey {
    /// Access the raw key value. Every call site is auditable via `grep expose_secret`.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl std::fmt::Display for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl Serialize for ApiKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

impl<'de> Deserialize<'de> for ApiKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Delegate to secrecy's built-in SecretString Deserialize impl
        SecretString::deserialize(deserializer).map(ApiKey)
    }
}
```

### Key Design Properties

1. **Debug safety:** `derive(Debug)` on ProviderConfig automatically picks up `ApiKey`'s Debug impl, producing `api_key: Some([REDACTED])` or `api_key: [REDACTED]`
2. **Serialize safety:** Custom Serialize always emits `"[REDACTED]"` -- impossible to accidentally serialize the real value
3. **Deserialize transparency:** TOML `api_key = "cashuA..."` deserializes into `ApiKey` because we delegate to secrecy's `SecretString::deserialize` which internally does `String::deserialize(d).map(Into::into)`
4. **Zeroize on drop:** Inherited from inner `SecretString` (which is `SecretBox<str>` with automatic `ZeroizeOnDrop`)
5. **Explicit access:** `.expose_secret()` returns `&str` -- every access point is a grep target

### Type Change Propagation

The type change flows through these structures:

```
ProviderConfig (src/config.rs)
  api_key: Option<String>  -->  Option<ApiKey>

SelectedProvider (src/router/selector.rs)
  api_key: Option<String>  -->  Option<ApiKey>

mock_config() (src/main.rs)
  api_key: None  -->  Some(ApiKey) for mock providers with keys, None otherwise

send_to_provider() (src/proxy/handlers.rs)
  format!("Bearer {}", api_key)  -->  format!("Bearer {}", api_key.expose_secret())

list_providers() (src/proxy/handlers.rs)
  Currently omits api_key  -->  Add "api_key": "[REDACTED]" when key present

providers command (src/main.rs)
  Currently prints api_key info  -->  Remove key column entirely
```

### Clarification on "api_key stays required"

The current codebase has `api_key: Option<String>` in `ProviderConfig`. The CONTEXT.md decision says "api_key stays required in this phase -- only the type changes." This means the **field** stays present with the **same optionality** (`Option<ApiKey>`), NOT that Option is removed. Evidence: mock providers use `api_key: None` and the handler code uses `if let Some(api_key) = &provider.api_key` to conditionally set the Authorization header -- providers without keys are a valid use case. The "required" wording means "don't change optionality in this phase; Phase 6 handles that."

The "Mock providers use SecretString too" decision means: mock providers that have keys should use `Some(ApiKey::from("mock-key"))`, and tests should verify that even mock keys get redacted. Mock providers that have no keys can remain `None`.

### Modified Files (Complete List)

```
src/config.rs          - Add ApiKey type, change ProviderConfig.api_key type
src/router/selector.rs - Change SelectedProvider.api_key type, update From impl
src/proxy/handlers.rs  - Use .expose_secret() in send_to_provider, add api_key to /providers JSON
src/main.rs            - Update mock_config(), update providers CLI command
Cargo.toml             - Add secrecy, remove config
```

### Anti-Patterns to Avoid
- **Using secrecy's SecretString directly on structs:** Debug prints `SecretBox<str>([REDACTED])` instead of just `[REDACTED]`, violating the user's redaction format decision
- **Implementing SerializableSecret for the inner type:** This would cause secrecy's Serialize to expose the actual secret -- the opposite of what we want
- **Using `#[serde(skip)]` on api_key:** Would omit the field from JSON entirely, but the decision requires `"api_key": "[REDACTED]"` to be present in the `/providers` endpoint response
- **Deriving Serialize on ProviderConfig with the new type:** This is actually fine because our custom ApiKey Serialize impl always produces `"[REDACTED]"` -- derive(Serialize) on the parent struct will use it correctly

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Zeroize-on-drop | Custom Drop impl with `ptr::write_volatile` | `secrecy::SecretString` (wraps zeroize crate) | Compiler can optimize away naive zeroing; zeroize uses compiler intrinsics |
| Debug redaction for each struct | Manual Debug impl on every struct that contains a key | Newtype with Debug impl + derive(Debug) on parent | Derive propagates correctly; manual impls are error-prone and don't compose |
| Serde deserialization from TOML string to secret type | Custom Visitor/Deserializer | secrecy's built-in `Deserialize` impl for `SecretString` | Handles all serde data formats (TOML, JSON, etc.) correctly |

**Key insight:** The secrecy crate exists precisely because hand-rolling secret management in Rust is deceptively hard -- the compiler can optimize away memory zeroing, and Debug/Display/Serialize leaks are easy to miss during code review.

## Common Pitfalls

### Pitfall 1: Forgetting `.expose_secret()` at the HTTP header site
**What goes wrong:** The Authorization header gets `Bearer [REDACTED]` instead of the actual key value.
**Why it happens:** After changing the type, `format!("Bearer {}", api_key)` calls Display, which now prints `[REDACTED]`.
**How to avoid:** Change to `format!("Bearer {}", api_key.expose_secret())`. This is the ONE place the real value should appear.
**Warning signs:** Integration tests or manual testing show 401 Unauthorized from providers.

### Pitfall 2: Tracing macros auto-formatting secrets
**What goes wrong:** `tracing::info!(config = ?config, ...)` calls Debug on the entire config, which would have shown secrets before but now shows `[REDACTED]`.
**Why it happens:** This is actually the DESIRED behavior. But verify no tracing calls do `config.providers[0].api_key.expose_secret()`.
**How to avoid:** Grep for `expose_secret` after implementation and verify every call site is intentional (should only be the Authorization header).
**Warning signs:** `grep -r "expose_secret" src/` shows more than 1-2 call sites.

### Pitfall 3: Clone semantics change
**What goes wrong:** `ProviderConfig.clone()` still works because `ApiKey` derives Clone (via SecretString's Clone impl), but it clones the secret value. This is correct behavior but worth noting.
**Why it happens:** The `From<&ProviderConfig> for SelectedProvider` impl clones the api_key.
**How to avoid:** Nothing to avoid -- Clone is necessary for the current architecture. Just be aware it copies the secret to a new allocation (which also gets zeroized on drop).
**Warning signs:** None -- this is expected behavior.

### Pitfall 4: Test ProviderConfig construction becomes verbose
**What goes wrong:** Every test that constructs a `ProviderConfig` with `api_key: None` still works, but tests wanting a key value need `api_key: Some(ApiKey::from("test-key"))`.
**Why it happens:** Type changed from String to ApiKey.
**How to avoid:** Implement `From<&str>` and `From<String>` for `ApiKey` to keep construction ergonomic.
**Warning signs:** Many test compilation errors.

### Pitfall 5: `/providers` endpoint currently omits api_key entirely
**What goes wrong:** The current `list_providers` handler manually constructs JSON with `serde_json::json!()` and does not include api_key. The decision says it should include `"api_key": "[REDACTED]"` when a key is configured.
**Why it happens:** The current code was written before secret redaction was a concern.
**How to avoid:** Update the `list_providers` handler to include the api_key field using a conditional: if key is `Some`, include `"api_key": "[REDACTED]"`, if `None`, include `"api_key": null`.
**Warning signs:** Integration test for `/providers` endpoint doesn't check for api_key field.

### Pitfall 6: Accidental re-exposure through error messages
**What goes wrong:** Error messages in `send_to_provider` include provider name but could theoretically include key if someone adds it to error context.
**Why it happens:** Error handling code is often written quickly without considering what data gets stringified.
**How to avoid:** The type system protects us: `ApiKey`'s Display impl prints `[REDACTED]`, so even accidental inclusion in error messages is safe. But still audit error messages to ensure they only reference provider names.
**Warning signs:** `grep -r "api_key" src/error.rs` returns any matches.

## Code Examples

### Creating an ApiKey from string (construction helpers)

```rust
// Source: custom pattern for ergonomic construction
impl From<String> for ApiKey {
    fn from(s: String) -> Self {
        ApiKey(SecretString::from(s))
    }
}

impl From<&str> for ApiKey {
    fn from(s: &str) -> Self {
        ApiKey(SecretString::from(s))
    }
}
```

### Accessing the secret for HTTP Authorization

```rust
// Source: src/proxy/handlers.rs (modified)
// BEFORE:
if let Some(api_key) = &provider.api_key {
    upstream_request =
        upstream_request.header(header::AUTHORIZATION, format!("Bearer {}", api_key));
}

// AFTER:
if let Some(api_key) = &provider.api_key {
    upstream_request =
        upstream_request.header(header::AUTHORIZATION, format!("Bearer {}", api_key.expose_secret()));
}
```

### Updated ProviderConfig with derive(Debug) propagation

```rust
// Source: src/config.rs (modified)
/// Provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    pub url: String,
    pub api_key: Option<ApiKey>,  // Changed from Option<String>
    #[serde(default)]
    pub models: Vec<String>,
    // ... rest unchanged
}

// Debug output now automatically shows:
// ProviderConfig { name: "alpha", url: "...", api_key: Some([REDACTED]), ... }
```

### Updated /providers endpoint with redacted key

```rust
// Source: src/proxy/handlers.rs (modified)
pub async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    let providers: Vec<serde_json::Value> = state
        .router
        .providers()
        .iter()
        .map(|p| {
            let mut provider_json = serde_json::json!({
                "name": p.name,
                "models": p.models,
                "input_rate_sats_per_1k": p.input_rate,
                "output_rate_sats_per_1k": p.output_rate,
                "base_fee_sats": p.base_fee,
            });
            // Include redacted api_key field
            if p.api_key.is_some() {
                provider_json["api_key"] = serde_json::json!("[REDACTED]");
            } else {
                provider_json["api_key"] = serde_json::Value::Null;
            }
            provider_json
        })
        .collect();

    Json(serde_json::json!({
        "providers": providers
    }))
}
```

### Updated mock_config with SecretString keys

```rust
// Source: src/main.rs (modified)
use arbstr::config::ApiKey;

ProviderConfig {
    name: "mock-cheap".to_string(),
    url: "http://localhost:9999/v1".to_string(),
    api_key: Some(ApiKey::from("mock-test-key-cheap")),  // Changed from None
    // ... rest unchanged
}
```

### Test: Verify Debug redaction

```rust
#[test]
fn test_api_key_debug_redaction() {
    let key = ApiKey::from("super-secret-cashu-token");
    let debug_output = format!("{:?}", key);
    assert_eq!(debug_output, "[REDACTED]");
    assert!(!debug_output.contains("super-secret"));
}

#[test]
fn test_provider_config_debug_redaction() {
    let config = ProviderConfig {
        name: "test".to_string(),
        url: "https://example.com/v1".to_string(),
        api_key: Some(ApiKey::from("cashuABCD1234secret")),
        models: vec![],
        input_rate: 10,
        output_rate: 30,
        base_fee: 1,
    };
    let debug_output = format!("{:?}", config);
    assert!(debug_output.contains("[REDACTED]"));
    assert!(!debug_output.contains("cashuABCD1234secret"));
}

#[test]
fn test_api_key_serialize_redaction() {
    let key = ApiKey::from("real-secret-value");
    let json = serde_json::to_string(&key).unwrap();
    assert_eq!(json, "\"[REDACTED]\"");
    assert!(!json.contains("real-secret"));
}

#[test]
fn test_api_key_deserialize_from_string() {
    let key: ApiKey = serde_json::from_str("\"my-secret-key\"").unwrap();
    assert_eq!(key.expose_secret(), "my-secret-key");
}

#[test]
fn test_api_key_expose_secret() {
    let key = ApiKey::from("the-actual-value");
    assert_eq!(key.expose_secret(), "the-actual-value");
}
```

### Updated CLI providers command (key column removed)

```rust
// Source: src/main.rs (modified)
Commands::Providers { config: config_path } => {
    let config = Config::from_file(&config_path)?;

    if config.providers.is_empty() {
        println!("No providers configured.");
    } else {
        println!("Configured providers:\n");
        for provider in &config.providers {
            println!("  {} ({})", provider.name, provider.url);
            if !provider.models.is_empty() {
                println!("    Models: {}", provider.models.join(", "));
            }
            println!(
                "    Rates: {} sats/1k input, {} sats/1k output",
                provider.input_rate, provider.output_rate
            );
            if provider.base_fee > 0 {
                println!("    Base fee: {} sats", provider.base_fee);
            }
            // No api_key output -- a column of identical [REDACTED] adds no value
            println!();
        }
    }
    Ok(())
}
```

Note: The current CLI providers command already does NOT print api_key info, so no change is needed for the CLI output format. The current code is already correct for this decision.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `secrecy` v0.8 `Secret<String>` | `secrecy` v0.10 `SecretBox<str>` (alias: `SecretString`) | v0.9/v0.10 (2023) | Type alias changed; `Secret<T>` is now `SecretBox<T>`; Debug format changed from `Secret([REDACTED])` to `SecretBox<T>([REDACTED])` |
| `secrecy` default Serialize exposed value | Serialize requires `SerializableSecret` marker | v0.10 | Serialize is opt-in per type, preventing accidental serialization of secrets |

**Deprecated/outdated:**
- `secrecy` v0.8 `Secret<String>` type: Replaced by `SecretBox<str>` (`SecretString`) in v0.10. Many blog posts and examples still reference the old type.
- The `config` crate (v0.14) in this project's Cargo.toml: Never used in source code, should be removed.

## Open Questions

1. **Should `Option` be kept for api_key?**
   - What we know: Current code uses `Option<String>`. CONTEXT says "api_key stays required." Mock providers use `api_key: None`. The handler conditionally sends Authorization header.
   - What's unclear: "Required" could mean "the field is required in TOML" or "keep the field as-is (Option)."
   - Recommendation: **Keep as `Option<ApiKey>`**. The handler logic requires optionality (providers without keys are valid). The CONTEXT note "Mock providers use SecretString too" means mock providers that HAVE keys should use ApiKey, not that all providers must have keys. Phase 6 will handle convention-based auto-discovery of keys for providers that omit them. Changing to required would break the mock providers that intentionally have no keys and the `if let Some(api_key)` pattern in the handler.

2. **Where to put the `ApiKey` type?**
   - What we know: It's used in config.rs, selector.rs, handlers.rs, and main.rs.
   - What's unclear: Whether it belongs in config.rs or a new module.
   - Recommendation: **Put in `src/config.rs`** alongside `ProviderConfig` that uses it. It's a config-layer type. Re-export from lib.rs if needed. No need for a separate module for a single type.

## Sources

### Primary (HIGH confidence)
- [docs.rs/secrecy/0.10.3](https://docs.rs/secrecy/0.10.3/secrecy/) - Full API docs, SecretBox source code, Debug impl format string, serde impls
- [docs.rs/secrecy/0.10.3/src/secrecy/lib.rs.html](https://docs.rs/secrecy/0.10.3/src/secrecy/lib.rs.html) - Exact source code showing `write!(f, "SecretBox<{}>([REDACTED])", any::type_name::<S>())` Debug format
- [GitHub iqlusioninc/crates secrecy/Cargo.toml](https://raw.githubusercontent.com/iqlusioninc/crates/main/secrecy/Cargo.toml) - Version 0.10.3, serde feature config, zeroize dependency

### Secondary (MEDIUM confidence)
- [Rust forum: Secrecy Crate Serialize String](https://users.rust-lang.org/t/secrecy-crate-serialize-string/112263) - Community pattern for newtype wrapper with SerializableSecret
- [Leapcell: Secure Configuration in Rust with Secrecy](https://leapcell.io/blog/secure-configuration-and-secrets-management-in-rust-with-secrecy-and-environment-variables) - Practical SecretString + serde usage pattern (note: uses v0.8 examples)

### Codebase (HIGH confidence)
- `src/config.rs` - Current `ProviderConfig` with `api_key: Option<String>`, derive(Debug, Clone, Deserialize)
- `src/router/selector.rs` - `SelectedProvider` with `api_key: Option<String>`, From impl
- `src/proxy/handlers.rs` - `send_to_provider` uses `provider.api_key` for Authorization header; `list_providers` constructs JSON manually
- `src/main.rs` - `mock_config()` uses `api_key: None`; `providers` command doesn't print api_key
- `Cargo.toml` - `config = "0.14"` listed but never used in source

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - secrecy v0.10.3 is verified from docs.rs, serde feature confirmed in source
- Architecture: HIGH - newtype pattern is well-established, Debug format string verified from source
- Pitfalls: HIGH - based on direct source code analysis of all api_key usage sites in the codebase

**Research date:** 2026-02-15
**Valid until:** 90 days (secrecy crate is stable, no breaking changes expected)
