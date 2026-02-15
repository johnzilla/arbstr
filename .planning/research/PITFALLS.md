# Domain Pitfalls: Secrets Handling in Existing Rust Proxy

**Domain:** Adding secrets handling to an existing Rust/Tokio/axum application
**Researched:** 2026-02-15
**Scope:** Env var expansion, key redaction across Debug/tracing/endpoints/errors
**Confidence:** HIGH (based on direct codebase analysis + verified library documentation)

---

## Critical Pitfalls

Mistakes that expose secrets in production or require architectural rework to fix.

---

### Pitfall 1: `#[derive(Debug)]` on Config Structs Exposes API Keys in Logs

**What goes wrong:** Every config struct in this codebase has `#[derive(Debug, Clone, Deserialize)]`. The `ProviderConfig` struct (config.rs:52) contains `api_key: Option<String>`. Any code path that formats a `ProviderConfig` with `{:?}` -- including panics, `tracing::debug!`, `unwrap()` failures, and `assert!` messages in tests -- will print the full API key in plaintext. The `Config`, `Router`, `SelectedProvider`, and `AppState` structs all transitively contain `api_key` through `ProviderConfig`, so `Debug`-formatting any of them leaks keys.

**Why it happens:** `#[derive(Debug)]` is a Rust reflex. It is on every struct in the codebase. Changing `api_key` from `String` to `SecretString` (from the `secrecy` crate) would break compilation everywhere `Debug` is derived, because `SecretString`'s `Debug` impl prints `[REDACTED]`. But if you only wrap the field without updating all the structs that clone/move/access it, you get a partially protected system that still leaks through transitive `Debug` on parent structs.

**Specific leak surfaces in this codebase:**
- `ProviderConfig` has `#[derive(Debug)]` (config.rs:52)
- `SelectedProvider` has `#[derive(Debug, Clone)]` and `api_key: Option<String>` (selector.rs:9-17)
- `Router` has `#[derive(Debug, Clone)]` and stores `Vec<ProviderConfig>` (selector.rs:33-38)
- `Config` has `#[derive(Debug, Clone)]` (config.rs:7)
- `AppState` stores `Arc<Config>` (server.rs:29) -- if anyone `Debug`-formats the state, all keys leak

**Consequences:**
- A single `tracing::debug!(config = ?config, ...)` or `dbg!(&config)` dumps all API keys
- Panic backtraces that include config values expose keys in crash logs
- Test assertion failures that print the struct expose keys in CI output

**Warning signs:**
- `#[derive(Debug)]` on any struct containing secrets
- No grep for `debug` or `?` formatting of config/provider/router types
- Tests that construct `ProviderConfig` with real-looking API keys

**Prevention:**
1. Replace `api_key: Option<String>` with `api_key: Option<SecretString>` in `ProviderConfig` and `SelectedProvider`. The `secrecy` crate's `SecretString` implements `Debug` as `[REDACTED]`. Use `secrecy = { version = "0.10", features = ["serde"] }` to keep serde Deserialize working.
2. Access the actual key value only at the single point it is needed: the `Authorization` header construction in `send_to_provider` (handlers.rs:498-501). Call `.expose_secret()` only there.
3. Audit every `#[derive(Debug)]` on structs that transitively contain `api_key`. With `SecretString`, the derived `Debug` will automatically redact -- but verify this with a test.
4. Add a test that `Debug`-formats a `ProviderConfig` with a known key and asserts the key does NOT appear in the output.

**Detection:** `grep -rn 'derive.*Debug' src/` on any struct containing a field named `key`, `secret`, `token`, or `password`. If the field type is `String`, it is exposed.

**Which phase should address it:** First phase. This is the foundation -- all other redaction work is pointless if `Debug` still leaks.

**Confidence:** HIGH -- directly observable in config.rs:52 and selector.rs:9-17.

---

### Pitfall 2: `Clone` on Secret-Bearing Structs Creates Untracked Copies

**What goes wrong:** `ProviderConfig` and `SelectedProvider` both derive `Clone`, and the api_key is `Option<String>`. Every `.clone()` creates a new heap allocation containing the plaintext key. These clones are scattered throughout the codebase:
- `ProviderConfig` is cloned into `SelectedProvider` via the `From` impl (selector.rs:19-29)
- Provider configs are cloned when building the router (server.rs:80-81: `config.providers.clone()`)
- `SelectedProvider` is returned from `select()` and moved into various handler scopes

With plain `String`, each clone is an independent heap allocation that lives until it is dropped. The original config, the router's copy, and the selected provider's copy all hold the key simultaneously. None of these locations zeroes memory on drop.

**Why it happens:** Rust's ownership model means moving values does a memcpy under the hood. The compiler may optimize some moves away, but cannot guarantee it. When a `String` containing a key is cloned, moved, or dropped, the previous memory location may retain the key bytes until the allocator reuses the page. This is a known Rust security concern documented in the zeroize crate literature.

**Consequences:**
- Multiple copies of each API key exist in process memory at any given time
- Dropped copies leave key material in freed heap memory
- A core dump, `/proc/[pid]/mem` read, or memory-scanning attack reveals keys even after the structs are dropped

**Warning signs:**
- `#[derive(Clone)]` on structs containing `String` secrets
- Multiple ownership paths for the same secret value
- No zeroize-on-drop for secret-bearing types

**Prevention:**
1. Use `SecretString` from the `secrecy` crate, which zeros memory on drop via the `zeroize` crate. `SecretString` also implements `Clone` (via `CloneableSecret`), but at least each copy is zeroed on drop.
2. Minimize the number of copies. Consider storing api_key in a shared `Arc<SecretString>` rather than cloning the string into every `SelectedProvider`. The current `From<&ProviderConfig>` impl (selector.rs:19-29) clones the key -- this could reference the original instead.
3. Accept that Rust cannot guarantee zero copies in memory (buffer reallocation during string construction, stack spills, etc.). `SecretString` is a best-effort mitigation, not a guarantee. For this application (a local proxy, not a hardware security module), best-effort zeroization is sufficient.

**Detection:** Search for `.clone()` calls on types containing secrets. Count the number of independent copies of each key that exist at runtime.

**Which phase should address it:** Same phase as Pitfall 1 -- the `SecretString` migration handles both Debug redaction and zeroize-on-drop simultaneously.

**Confidence:** HIGH -- Clone derives visible at config.rs:52 and selector.rs:9. Memory zeroing concern is well-documented in the secrecy/zeroize ecosystem.

---

### Pitfall 3: `/providers` Endpoint Returns Config Struct That May Include Keys

**What goes wrong:** The `/providers` endpoint (handlers.rs:765-784) manually constructs JSON from `router.providers()`, and currently does NOT include `api_key`. However, the underlying `router.providers()` method (selector.rs:196) returns `&[ProviderConfig]`, which includes the `api_key` field. If anyone changes the JSON construction to use `serde_json::to_value(p)` instead of the manual `json!({...})` macro, all fields -- including `api_key` -- will be serialized and returned to the caller.

This is a latent vulnerability. The current code is safe only because of the manual field selection in the `json!` macro. There is no type-level protection preventing serialization of the key.

**Why it happens:** The `ProviderConfig` struct derives `Deserialize` (for config loading) but does not derive `Serialize`. This provides some protection today -- you cannot accidentally `serde_json::to_value()` a `ProviderConfig`. But if someone adds `#[derive(Serialize)]` for any reason (e.g., a config export command, a debug endpoint), all fields including `api_key` become serializable.

**Consequences:**
- A future code change could expose all API keys via a public HTTP endpoint
- Anyone with network access to the proxy could read all provider credentials

**Warning signs:**
- `#[derive(Serialize)]` added to `ProviderConfig`
- `serde_json::to_value()` called on a struct containing secrets
- JSON construction that uses struct serialization instead of manual field selection

**Prevention:**
1. Using `SecretString` for `api_key` provides automatic protection: the `secrecy` crate intentionally does NOT implement `Serialize` for `SecretString` by default. Even if `ProviderConfig` gains `#[derive(Serialize)]`, the `api_key: Option<SecretString>` field will cause a compilation error, forcing the developer to consciously handle it.
2. Create a separate `ProviderInfo` struct (without `api_key`) for API responses. This struct should be the return type of a dedicated method, not a filtered view of `ProviderConfig`.
3. Add a test that hits `/providers` and asserts the response does NOT contain any string matching the configured API keys.

**Detection:** Test that serializes a `ProviderConfig` and asserts the key is absent. Grep for `Serialize` on config types.

**Which phase should address it:** Second phase (after SecretString migration). The type-level protection from `SecretString` makes accidental serialization a compile error rather than a runtime bug.

**Confidence:** HIGH -- handler code at handlers.rs:765-784 and struct definition at config.rs:52 directly observable.

---

### Pitfall 4: Error Messages Include Provider URLs and Context That Leak Keys

**What goes wrong:** Several error paths in the codebase format provider details into error messages that are returned to clients via the OpenAI-compatible error response:

1. `send_to_provider` (handlers.rs:506-508): `"Failed to reach provider '{}': {}"` -- the reqwest error may include the full URL with query parameters
2. `send_to_provider` (handlers.rs:526-529): `"Provider '{}' returned {}: {}"` -- the error body from the provider may contain credential-related information
3. `Error::Provider(String)` (error.rs:23) passes the string directly to the JSON error response via `self.to_string()` in `IntoResponse` (error.rs:43)

If the provider URL contains credentials (e.g., `https://user:pass@provider.com/v1`) or if the provider's error response mentions the API key (e.g., "Invalid API key: cashuA1234..."), that information flows directly to the client in the error response.

**Why it happens:** Error messages are constructed for debugging convenience, not security. The pattern `format!("Failed to reach provider '{}': {}", provider.name, e)` is natural for development but dangerous in production when the error is returned to HTTP clients.

**Consequences:**
- API keys or partial key values appear in HTTP error responses
- Provider URLs (which may contain credentials) appear in client-facing errors
- Error aggregation services (Sentry, etc.) store secrets in plaintext

**Warning signs:**
- `format!` or `Display` on error types that include external error messages
- Provider error bodies forwarded to clients without sanitization
- `self.to_string()` used in `IntoResponse` for error types that may contain sensitive context

**Prevention:**
1. Separate internal error messages (for logging) from external error messages (for HTTP responses). The `IntoResponse` impl should return a generic message, while the full detail is logged server-side.
2. Never include the raw reqwest error in client-facing responses. The reqwest error may contain the request URL. Log it at `tracing::error!` level, but return `"Failed to reach provider"` to the client.
3. Never forward provider error bodies verbatim to clients. The provider may echo back credentials or internal details. Log the body server-side, return `"Provider returned an error"` to the client.
4. Audit every `Error::Provider(format!(...))` call site and strip sensitive context.

**Detection:** Grep for `Error::Provider(format!` and examine what data flows into the format string. Check if any external/untrusted content reaches the client-facing error message.

**Which phase should address it:** Should be addressed alongside the SecretString migration, but is a separate concern. Even with SecretString, error messages can leak keys if they include raw upstream error text.

**Confidence:** HIGH -- error construction visible at handlers.rs:506-508 and handlers.rs:526-529, error rendering at error.rs:38-60.

---

## Moderate Pitfalls

Mistakes that create incomplete protection or subtle leak paths.

---

### Pitfall 5: `tracing::info!` Logs Provider URLs (Which May Contain Credentials)

**What goes wrong:** The `execute_request` function (handlers.rs:576-581) logs:
```rust
tracing::info!(
    provider = %provider.name,
    url = %provider.url,
    output_rate = %provider.output_rate,
    "Selected provider"
);
```

The `url` field is logged at `info` level, which is the default log level. If a provider URL contains credentials (common in some API patterns, e.g., basic auth embedded in URL), those credentials appear in every request log. Even without embedded credentials, the URL reveals the provider's API endpoint, which is potentially sensitive information about the user's infrastructure.

Additionally, `tower_http::TraceLayer` (server.rs:54-69) logs request URIs, which would include any query parameters. While arbstr's own endpoints are clean, this middleware pattern logs everything.

**Why it happens:** URL logging is standard practice for HTTP proxies. The risk is specific to URLs that contain credentials, which is a pattern arbstr does not currently use but cannot prevent users from configuring.

**Prevention:**
1. Remove `url` from the `tracing::info!` in `execute_request`. The provider name is sufficient for operational debugging. The URL can be logged at `debug` level if needed.
2. If URL logging is retained, redact it: log only the host portion, not the full URL (which may contain path-based tokens or query credentials).
3. Consider adding a `tracing::info!` filter or a custom `MakeSpan` that strips sensitive headers and URL components.

**Warning signs:**
- `url = %` in any tracing macro at info level or above
- Full URLs logged without redaction

**Which phase should address it:** Same phase as the SecretString migration. Review all tracing calls as part of the redaction audit.

**Confidence:** HIGH -- log statement directly visible at handlers.rs:576-581.

---

### Pitfall 6: Env Var Expansion That Fails Open (Missing Var Returns Empty String)

**What goes wrong:** When implementing `${ENV_VAR}` expansion in TOML config values, a common mistake is treating a missing environment variable as an empty string rather than an error. If `api_key = "${ROUTSTR_KEY}"` and `ROUTSTR_KEY` is not set, the key becomes an empty string. The proxy then sends requests to the provider with `Authorization: Bearer ` (empty token), which:
- Fails with a confusing 401 error from the provider
- May be logged as "authentication failed" with no indication that the key was misconfigured
- In the worst case, if the provider treats empty auth as "no auth" and allows unauthenticated access, requests proceed without payment tracking

**Why it happens:** `std::env::var("KEY").unwrap_or_default()` returns empty string for missing vars. Regex-based substitution that replaces `${VAR}` with the env value naturally produces empty string for missing vars.

**Consequences:**
- Silent misconfiguration that manifests as confusing auth errors
- Config validation passes (the api_key field is present), but the value is useless
- Users think the proxy is broken when the real issue is a missing environment variable

**Warning signs:**
- Env var lookup with `unwrap_or_default()` or `.unwrap_or("".to_string())`
- No distinction between "variable is set to empty" and "variable is not set"
- Config validation that checks field presence but not value content

**Prevention:**
1. Treat `${VAR}` where VAR is unset as a hard error during config loading. Fail fast with a clear message: "Environment variable 'ROUTSTR_KEY' not set, referenced in providers[0].api_key"
2. Distinguish between `${VAR}` (must be set, error if missing) and `${VAR:-default}` (use default if missing) syntax
3. Validate expanded values: an api_key that is empty after expansion should be treated the same as a missing api_key
4. Log which env vars were resolved (by name, not value) at startup, so operators can verify the config

**Detection:** Config test that sets an env var reference but does not set the variable, and asserts the parse fails with a descriptive error.

**Which phase should address it:** First phase of env var expansion. The error handling for missing vars must be designed before the expansion logic.

**Confidence:** HIGH -- standard software engineering concern, well-understood failure mode.

---

### Pitfall 7: Convention-Based Env Var Lookup Conflicts with Explicit Config

**What goes wrong:** The v1.1 milestone specifies two mechanisms: explicit `api_key = "${ROUTSTR_KEY}"` in TOML, and convention-based `ARBSTR_<PROVIDER>_API_KEY` auto-detection when `api_key` is omitted. If both are present -- a user sets `api_key = "hardcoded_key"` in TOML AND has `ARBSTR_MYPROVIDER_API_KEY` in their environment -- which one wins? Without a clear, documented precedence rule, behavior is unpredictable and debugging is difficult.

**Why it happens:** The two mechanisms are designed for different use cases (explicit config vs zero-config convenience), but nothing prevents them from overlapping. The implementation order matters: if convention lookup runs after TOML parsing, it may overwrite an explicitly configured key.

**Consequences:**
- User sets a key in config, but the convention env var silently overrides it (or vice versa)
- Debugging auth failures becomes harder because the actual key used is not the one the user expects
- Security risk: a stale env var from a previous session could override a fresh config value

**Warning signs:**
- No documented precedence order
- Convention lookup that runs unconditionally even when an explicit key is present
- No startup log indicating which key source was used for each provider

**Prevention:**
1. Define and document a clear precedence: explicit TOML value > `${ENV}` expansion in TOML > convention-based `ARBSTR_<NAME>_API_KEY` > no key
2. Convention-based lookup should ONLY apply when `api_key` is not present in the TOML config at all (not when it is present but empty)
3. Log the key source (not value) at startup for each provider: `"Provider 'alpha' api_key: from config file"`, `"Provider 'beta' api_key: from env ARBSTR_BETA_API_KEY"`, `"Provider 'gamma' api_key: not configured"`
4. Add a config validation test that verifies precedence order with all combinations

**Detection:** Integration test that sets both explicit config and convention env var, verifies the expected one wins.

**Which phase should address it:** Must be designed before implementing convention-based lookup. The precedence rule is an architectural decision, not an implementation detail.

**Confidence:** MEDIUM -- depends on implementation approach, but precedence conflicts are a common pattern in config systems.

---

### Pitfall 8: `SecretString` Breaks Existing Serde Deserialization for TOML

**What goes wrong:** Changing `api_key: Option<String>` to `api_key: Option<SecretString>` in `ProviderConfig` requires the `secrecy` crate's `serde` feature. With the feature enabled, `SecretString` implements `Deserialize`, so `toml::from_str` can parse it. However, there are subtle compatibility issues:

1. `SecretString` is `SecretBox<str>`, which deserializes a `Box<str>` internally. TOML string values work fine, but if anyone passes a non-string TOML type (integer, boolean) where a key is expected, the error message will reference `SecretBox` internal types, confusing users.
2. The existing tests in `config.rs` (lines 194-249) construct `ProviderConfig` without api_key. Any test that constructs one WITH an api_key will need to use `SecretString::from("value")` instead of just `"value".to_string()`.
3. The `SelectedProvider::from(&ProviderConfig)` impl clones the api_key. `SecretString` implements `Clone`, so this works -- but the clone semantics are different (zeroize-on-drop applies to each copy).

**Why it happens:** Type changes in core config structs ripple through the entire codebase. `SecretString` is intentionally not a drop-in replacement for `String` -- it has restricted APIs to prevent accidental exposure. Code that previously called `.as_ref()`, `.as_str()`, or compared keys with `==` will not compile.

**Prevention:**
1. Make the change incrementally: first add `secrecy` dependency with serde feature, then change the type in `ProviderConfig`, fix all compilation errors, then change `SelectedProvider`, fix all compilation errors.
2. The only place that needs the actual string value is `send_to_provider` (handlers.rs:498-501) where it constructs the `Authorization` header. Use `api_key.expose_secret()` there.
3. Update all tests that construct providers with api_keys to use `SecretString::from(...)`.
4. If `Option<SecretString>` causes too much friction (e.g., `#[serde(default)]` behavior, None handling), consider a newtype wrapper around `Option<SecretString>` with convenience methods.

**Detection:** `cargo build` after the type change will show every location that needs updating. This is Rust's type system working as intended -- treat compilation errors as a checklist.

**Which phase should address it:** First phase. This is purely a mechanical migration, but it touches many files and must be done carefully to avoid partial application.

**Confidence:** HIGH -- directly follows from changing `String` to `SecretString` in Rust's type system.

---

## Minor Pitfalls

Mistakes that cause confusion or minor issues but are easily fixed.

---

### Pitfall 9: Provider Name Normalization for Convention-Based Env Vars

**What goes wrong:** Convention-based lookup maps provider name `"provider-alpha"` to env var `ARBSTR_PROVIDER_ALPHA_API_KEY`. But provider names can contain characters that are invalid in env var names (hyphens, dots, spaces). The normalization rule (e.g., replace `-` with `_`, uppercase everything) must be clearly defined and consistently applied.

Edge cases: `"my.provider"` -> `ARBSTR_MY_PROVIDER_API_KEY` (dot to underscore?), `"Provider Alpha"` -> `ARBSTR_PROVIDER_ALPHA_API_KEY` (space to underscore?), `"provider_alpha"` and `"provider-alpha"` -> both map to the same env var (collision).

**Prevention:**
1. Document the exact normalization: uppercase, replace non-alphanumeric with underscore, collapse consecutive underscores
2. Detect collisions at startup: if two provider names normalize to the same env var name, warn or error
3. Log the expected env var name for each provider at startup so users know exactly what to set

**Which phase should address it:** Implementation phase of convention-based lookup.

**Confidence:** MEDIUM -- depends on the variety of provider names users configure.

---

### Pitfall 10: Env Var Expansion Only Applied to `api_key` Field

**What goes wrong:** If env var expansion is implemented only for the `api_key` field (because that is the current use case), users may expect it to work for other fields too -- like `url` (which might reference `${ROUTSTR_URL}`). Implementing field-specific expansion creates an inconsistent experience. Implementing generic expansion (any string field can use `${VAR}`) is more work but more predictable.

**Prevention:**
1. Decide upfront: is env var expansion a feature of the TOML parser (applies to all string values) or a feature of specific fields (only api_key)?
2. If field-specific: document which fields support expansion and which do not.
3. If generic: apply expansion to ALL string values in the config after TOML parsing but before validation. This is simpler to explain and implement (single pass over all strings).
4. Either way: expansion should happen BEFORE config validation, so that `url = "${PROVIDER_URL}"` expanding to empty string fails URL validation.

**Which phase should address it:** Design phase of env var expansion.

**Confidence:** MEDIUM -- depends on user expectations and design choices.

---

### Pitfall 11: Test Fixtures That Contain Real-Looking Secrets

**What goes wrong:** Test code (selector.rs tests, handlers.rs tests, config.rs tests) constructs `ProviderConfig` with `api_key: None` or no api_key at all. When secrets handling is added, tests will need realistic api_key values. If someone uses a real key format (e.g., `api_key: Some("cashuA1REAL_LOOKING_TOKEN...")`) in test fixtures, those values may:
- Appear in CI logs
- Be committed to version control
- Be mistaken for real credentials by secret scanning tools

**Prevention:**
1. Use obviously fake values in tests: `SecretString::from("test-key-not-real")`
2. Add a `.gitignore` or pre-commit hook for common secret patterns
3. If using the `secrecy` crate, test that `Debug`-formatting a test provider does NOT show the key -- this doubles as both a redaction test and a safety net for CI output

**Which phase should address it:** Concurrent with the SecretString migration -- tests need updating anyway.

**Confidence:** HIGH -- test construction visible throughout the test modules.

---

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|---|---|---|
| SecretString migration | Debug derive leaking keys (Pitfall 1) | Replace `String` with `SecretString`, verify with Debug-format test |
| SecretString migration | Clone proliferating key copies (Pitfall 2) | SecretString zeroizes on drop; minimize copies via Arc |
| SecretString migration | Serde compatibility breaks (Pitfall 8) | Enable secrecy serde feature, update tests incrementally |
| Env var expansion | Failing open on missing vars (Pitfall 6) | Treat missing vars as hard errors, fail fast at config load |
| Env var expansion | Field scope confusion (Pitfall 10) | Decide generic vs field-specific upfront |
| Convention env var lookup | Precedence conflicts (Pitfall 7) | Document and enforce: explicit > expanded > convention |
| Convention env var lookup | Name normalization edge cases (Pitfall 9) | Document rules, detect collisions, log expected var names |
| Endpoint/error redaction | Error messages leaking keys (Pitfall 4) | Separate internal log messages from client-facing errors |
| Endpoint/error redaction | /providers serialization risk (Pitfall 3) | SecretString blocks Serialize; add endpoint response test |
| Endpoint/error redaction | tracing leaking URLs (Pitfall 5) | Audit all tracing calls, remove URL from info level |
| Testing | Test fixtures with real-looking secrets (Pitfall 11) | Use obviously fake values, test Debug redaction |

## Recommended Phase Ordering Based on Pitfalls

The dependency chain for secrets handling:

1. **SecretString type migration** (Pitfalls 1, 2, 3, 8) -- Foundation. Changes the type of `api_key` from `String` to `SecretString` across `ProviderConfig`, `SelectedProvider`, and related code. This is a mechanical migration guided by compiler errors. Provides automatic Debug redaction and Serialize protection. Must come first because all subsequent work builds on the new type.

2. **Env var expansion** (Pitfalls 6, 7, 10) -- Configuration. Implements `${VAR}` syntax in TOML string values and convention-based `ARBSTR_<NAME>_API_KEY` lookup. Requires clear precedence rules and fail-fast on missing vars. Depends on phase 1 because expanded values must produce `SecretString`, not `String`.

3. **Output surface redaction audit** (Pitfalls 4, 5) -- Hardening. Reviews every code path where secret-adjacent data reaches external surfaces: error messages to HTTP clients, tracing calls at info level, response bodies. Depends on phases 1-2 because the type migration eliminates the most dangerous leak paths; this phase catches what the type system cannot enforce.

4. **Testing and validation** (Pitfall 11) -- Verification. Adds tests that verify redaction works across all surfaces. Integration tests that format providers, hit endpoints, trigger errors, and assert no key material appears. Can partially overlap with earlier phases.

## Existing Code Vulnerabilities (Direct Observations)

| File | Line(s) | Issue | Severity |
|---|---|---|---|
| `src/config.rs` | 52 | `#[derive(Debug)]` on `ProviderConfig` with `api_key: Option<String>` | CRITICAL |
| `src/router/selector.rs` | 9-10 | `#[derive(Debug, Clone)]` on `SelectedProvider` with `api_key: Option<String>` | CRITICAL |
| `src/router/selector.rs` | 33-34 | `#[derive(Debug, Clone)]` on `Router` containing `Vec<ProviderConfig>` | CRITICAL |
| `src/config.rs` | 7 | `#[derive(Debug)]` on `Config` containing `Vec<ProviderConfig>` | CRITICAL |
| `src/proxy/handlers.rs` | 506-508 | Error message includes reqwest error (may contain URL with credentials) | HIGH |
| `src/proxy/handlers.rs` | 526-529 | Error message includes provider error body (may echo credentials) | HIGH |
| `src/proxy/handlers.rs` | 576-581 | `tracing::info!` logs provider URL at default log level | MODERATE |
| `src/proxy/handlers.rs` | 765-784 | `/providers` endpoint safe today but no type-level protection against future `Serialize` addition | LOW (latent) |
| `src/proxy/server.rs` | 29 | `AppState` stores `Arc<Config>` -- Debug on AppState would leak all keys | MODERATE |

## Sources

- Direct codebase analysis of `/home/john/vault/projects/github.com/arbstr/src/` (all source files read)
- [secrecy crate documentation](https://docs.rs/secrecy/latest/secrecy/) -- SecretString, SecretBox, ExposeSecret trait, serde feature
- [Rust zeroize move/copy/drop pitfalls](https://benma.github.io/2020/10/16/rust-zeroize-move.html) -- memory copies surviving moves and drops
- [Secure Configuration and Secrets Management in Rust](https://leapcell.io/blog/secure-configuration-and-secrets-management-in-rust-with-secrecy-and-environment-variables) -- secrecy + serde integration patterns
- [tracing instrument macro](https://docs.rs/tracing/latest/tracing/attr.instrument.html) -- skip and skip_all for sensitive fields
- [veil crate for derive-based redaction](https://github.com/primait/veil) -- alternative to manual Debug impl
- [redaction crate](https://github.com/sformisano/redaction) -- context-based redaction patterns
- Confidence: HIGH overall. All critical pitfalls are directly observable in the codebase. Library behavior verified against official crate documentation.
