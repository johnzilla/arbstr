# Phase 7: Output Surface Hardening - Research

**Researched:** 2026-02-15
**Domain:** Unix file permission auditing, masked key prefix display, plaintext literal detection and warnings
**Confidence:** HIGH

## Summary

This phase implements three independent startup-time audits and one display-layer change across the `/providers` endpoint and `providers` CLI command. All three requirements (RED-01, RED-03, RED-04) build on infrastructure already established in Phases 5 and 6 -- the `ApiKey` newtype with `expose_secret()`, the `KeySource` enum, and the `from_file_with_env` loading pipeline with per-provider key source tracking.

**RED-01 (file permission warning)** requires checking `std::fs::metadata().permissions().mode()` via the `std::os::unix::fs::PermissionsExt` trait, guarded by `#[cfg(unix)]`. The check compares the file's mode bits (masked to `0o777`) against `0o600`; any additional bits set triggers a `tracing::warn!` at startup naming the file path and its octal permissions. This is purely advisory -- startup continues regardless.

**RED-03 (masked key prefix)** requires a new method on `ApiKey` (e.g., `masked_prefix()`) that returns a string like `cashuA...***` showing the first N characters of the key followed by a fixed mask. This replaces the current `"[REDACTED]"` string in the `/providers` JSON and is added to the `providers` CLI output. The method uses `expose_secret()` internally, so it remains grep-auditable. The Phase 5 CONTEXT.md explicitly noted: "Full `[REDACTED]` replacement everywhere -- no partial masks or prefixes (Phase 7 adds masked prefixes later)" -- this phase is exactly where masked prefixes are intended.

**RED-04 (literal key warning)** is nearly free: the `KeySource::Literal` variant from Phase 6 already identifies providers whose `api_key` was a plaintext literal in config (no `${}` expansion). The implementation is a `tracing::warn!` in the serve command's key source logging loop when `source == KeySource::Literal`, recommending environment variable usage.

**Primary recommendation:** Implement all three features as startup-time checks in `main.rs` (RED-01, RED-04) and a display method on `ApiKey` (RED-03), keeping the scope minimal. No new crate dependencies are needed. All three features are independently testable with unit tests and can be verified without running a full server.

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| std::os::unix::fs::PermissionsExt | stdlib | Read Unix file permission mode bits | Standard library trait, no external dependency |
| std::fs::metadata | stdlib | Get file metadata (permissions) for a path | Standard library function |
| tracing | 0.1 (already present) | Emit startup warnings | Already used throughout the project |
| secrecy | 0.10 (already present) | ApiKey.expose_secret() for masked prefix | Already used from Phase 5 |

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| serde_json | 1 (already present) | Update /providers JSON output | Already used in handlers.rs |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Manual mode bit check | `fs-mistrust` crate | fs-mistrust does full path-chain permission auditing (parent dirs, symlinks); massive overkill for a single file 0600 check; adds dependency |
| Manual prefix slicing | `mask-text` crate | mask-text adds flexible masking patterns; overkill for a single 6-char prefix + fixed mask; adds dependency |
| `#[cfg(unix)]` guard | `#[cfg(target_family = "unix")]` | Equivalent on current Rust; `cfg(unix)` is shorter and idiomatic |

**No new dependencies needed.** All three features use stdlib and existing project dependencies.

## Architecture Patterns

### Recommended Implementation Structure

```
src/
├── config.rs              # Add: ApiKey::masked_prefix() method, check_file_permissions() function
├── main.rs                # Update: serve command calls check_file_permissions() and warns on Literal keys
├── proxy/
│   └── handlers.rs        # Update: list_providers uses masked_prefix() instead of "[REDACTED]"
└── (no other files change)
```

### Pattern 1: Unix File Permission Check (RED-01)

**What:** A function that checks file permissions and returns a warning if they are more permissive than 0600.
**When to use:** Called once at startup after successfully loading the config file, before starting the server.
**Location:** `src/config.rs` (co-located with config loading logic).

```rust
// Source: std::os::unix::fs::PermissionsExt (https://doc.rust-lang.org/std/os/unix/fs/trait.PermissionsExt.html)

/// Check if a config file has permissions more permissive than 0600.
///
/// Returns `Some((path, mode))` if the file is overly permissive, `None` if OK.
/// On non-Unix platforms, always returns `None` (check is skipped).
#[cfg(unix)]
pub fn check_file_permissions(path: &std::path::Path) -> Option<(String, u32)> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).ok()?;
    let mode = metadata.permissions().mode() & 0o777;

    if mode & 0o177 != 0 {
        // Any bits beyond owner read/write are set
        Some((path.display().to_string(), mode))
    } else {
        None
    }
}

#[cfg(not(unix))]
pub fn check_file_permissions(_path: &std::path::Path) -> Option<(String, u32)> {
    None // Permission check not applicable on non-Unix
}
```

Key design decisions:
- `0o177` mask checks for group-read (0o040), group-write (0o020), group-execute (0o010), other-read (0o004), other-write (0o002), other-execute (0o001), and owner-execute (0o100). Any of these being set means the file is more permissive than 0600 (owner read+write only).
- Uses `& 0o777` to strip the file type bits from the mode, keeping only the permission bits.
- Returns `Option` rather than logging directly, so the caller controls the warning format and tests can verify without capturing log output.
- Guarded by `#[cfg(unix)]` / `#[cfg(not(unix))]` for cross-platform compilation. The non-Unix version is a no-op.
- `metadata().ok()?` silently returns `None` if the file can't be stat'd (e.g., already deleted between load and check). This is acceptable because the file was just successfully read.

### Pattern 2: Masked Key Prefix (RED-03)

**What:** A method on `ApiKey` that shows a recognizable prefix of the key followed by a mask.
**When to use:** In `/providers` endpoint JSON and `providers` CLI output, replacing the current `"[REDACTED]"` string.
**Location:** `src/config.rs` on the `ApiKey` impl.

```rust
impl ApiKey {
    /// Return a masked representation showing just enough prefix to identify the key.
    ///
    /// Format: first 6 characters + "...***"
    /// Example: "cashuA...***"
    ///
    /// For very short keys (<6 chars), shows fewer prefix chars.
    /// Uses expose_secret() internally -- grep-auditable.
    pub fn masked_prefix(&self) -> String {
        let secret = self.expose_secret();
        let prefix_len = secret.len().min(6);
        let prefix = &secret[..prefix_len];
        format!("{}...***", prefix)
    }
}
```

Design decisions:
- **6 characters** is enough to identify "cashuA" tokens (the standard Cashu token prefix) without revealing the actual token content. Cashu tokens are base64-encoded and typically 100+ characters, so 6 chars reveals less than 5% of the token.
- The `...***` suffix clearly indicates truncation and masking.
- For empty or very short keys (edge case), the method still works correctly -- it just shows fewer prefix characters.
- Uses `expose_secret()` internally, maintaining the grep-auditability property.
- This is a separate method from Debug/Display/Serialize, which continue to emit `[REDACTED]`. The masked prefix is opt-in, used only where the user explicitly wants to verify which key is loaded.

### Pattern 3: Literal Key Warning (RED-04)

**What:** A startup warning when a provider's key was loaded as a plaintext literal (no `${}` expansion).
**When to use:** During the key source logging loop in the `serve` command, right after config loading.
**Location:** `src/main.rs` in the `Commands::Serve` match arm.

```rust
for (provider_name, source) in &key_sources {
    match source {
        KeySource::Literal => {
            tracing::info!(provider = %provider_name, "key from config-literal");
            tracing::warn!(
                provider = %provider_name,
                "Plaintext API key in config file. \
                 Consider using environment variables: \
                 api_key = \"${{ARBSTR_{}_API_KEY}}\" or omit api_key and set the env var directly.",
                provider_name.to_uppercase().replace('-', "_").replace(' ', "_")
            );
        }
        // ... other variants unchanged
    }
}
```

Design decisions:
- The warning is advisory -- startup continues. Users may legitimately use literal keys during development.
- The warning includes the specific environment variable name the user should use, matching the convention from Phase 6. This makes the warning immediately actionable.
- Uses `tracing::warn!` (not `error!`) because this is a hygiene recommendation, not a failure.
- The same warning should also appear in the `check` command output for consistency.

### Pattern 4: Updated /providers Endpoint

**What:** Replace `"[REDACTED]"` with `masked_prefix()` output in the `/providers` JSON response.
**When to use:** In the `list_providers` handler.
**Location:** `src/proxy/handlers.rs`.

```rust
// Current (Phase 5):
"api_key": if p.api_key.is_some() {
    serde_json::Value::String("[REDACTED]".to_string())
} else {
    serde_json::Value::Null
},

// Updated (Phase 7):
"api_key": match &p.api_key {
    Some(key) => serde_json::Value::String(key.masked_prefix()),
    None => serde_json::Value::Null,
},
```

### Pattern 5: Updated providers CLI

**What:** Add masked key prefix to the CLI providers output.
**When to use:** In the `Commands::Providers` match arm.
**Location:** `src/main.rs`.

```rust
// Add key display to providers CLI output
if let Some(ref api_key) = provider.api_key {
    println!("    Key: {}", api_key.masked_prefix());
}
```

Note: Phase 5 decided "CLI `providers` command removes the key column entirely -- a column of identical `[REDACTED]` adds no value." Now that we have distinguishable masked prefixes (not identical `[REDACTED]` strings), showing the masked prefix in CLI output IS useful -- it lets users verify which key is loaded for each provider.

### Anti-Patterns to Avoid

- **Failing startup on permission issues:** RED-01 says "warn", not "fail". The warning is advisory. Some deployment environments (e.g., Docker) may have different permission models.
- **Showing too many characters in masked prefix:** 6 characters is enough for Cashu token identification. Showing more (e.g., 20 characters) would defeat the purpose of masking. Showing fewer (e.g., 2) wouldn't help identify the key.
- **Using the masked prefix in Debug/Display/Serialize impls:** These must continue to emit `[REDACTED]`. The masked prefix is a new, separate method for opt-in use in specific display contexts.
- **Checking permissions on the `from_file_with_env` path:** The permission check should NOT be inside `Config::from_file_with_env`. Config loading is a pure operation. Permission warnings are an operational concern that belongs in main.rs. This keeps the config module testable without filesystem side effects.
- **Duplicating convention_env_var_name logic:** The RED-04 warning message needs to suggest the convention env var name. Use the existing `convention_env_var_name()` function rather than duplicating the name transformation.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Unix permission bits reading | Custom libc FFI calls | `std::os::unix::fs::PermissionsExt::mode()` | Standard library provides this directly |
| Cross-platform permission check | Complex cfg tree per OS | `#[cfg(unix)]` / `#[cfg(not(unix))]` pair | Only Unix has meaningful file permissions for this use case |
| Key prefix extraction | Regex-based pattern matching | Simple `&secret[..prefix_len]` slice | Constant-length prefix, no pattern complexity |
| Convention env var name derivation | Inline string manipulation | `convention_env_var_name()` from Phase 6 | Already exists and is public |

**Key insight:** All three features are simple enough that they require zero external dependencies. The infrastructure from Phases 5 and 6 (ApiKey, KeySource, convention_env_var_name) provides all the building blocks needed.

## Common Pitfalls

### Pitfall 1: Permission check on non-existent or special paths
**What goes wrong:** `std::fs::metadata()` fails for paths that were deleted between config load and permission check, or for special paths like `/dev/stdin`.
**Why it happens:** The config file path might not be a regular file, or might be ephemeral.
**How to avoid:** Use `.ok()?` on the metadata call to silently skip the check if metadata can't be read. The file was just successfully loaded, so this is an edge case that doesn't warrant an error.
**Warning signs:** Panic or error on startup when config is piped via stdin or uses process substitution.

### Pitfall 2: Mode bits include file type bits
**What goes wrong:** `metadata.permissions().mode()` returns the full `st_mode` value including file type bits (e.g., `0o100644` for a regular file with 644 permissions). Comparing `mode > 0o600` would always be true.
**Why it happens:** The Unix `st_mode` field includes both file type and permission bits.
**How to avoid:** Always mask with `& 0o777` before comparing permission bits. Then check `mode & 0o177 != 0` to detect any bits beyond owner read/write.
**Warning signs:** Every file triggers the permission warning, even files with correct permissions.

### Pitfall 3: Masked prefix on empty or very short keys
**What goes wrong:** `&secret[..6]` panics if the key is shorter than 6 characters.
**Why it happens:** Edge case with malformed or placeholder keys.
**How to avoid:** Use `secret.len().min(6)` as the prefix length. Very short keys are unusual but shouldn't crash the application.
**Warning signs:** Panic when a provider has a very short API key.

### Pitfall 4: Masked prefix reveals too much for short keys
**What goes wrong:** For a 7-character key, `masked_prefix()` reveals 6 of 7 characters, effectively exposing the key.
**Why it happens:** The fixed 6-character prefix is too large relative to the key length.
**How to avoid:** For keys shorter than a threshold (e.g., 10 characters), show fewer prefix characters or fall back to `[REDACTED]`. Cashu tokens are typically 100+ characters, so this is unlikely in practice but worth handling defensively.
**Warning signs:** Short test keys being effectively exposed in masked output.

### Pitfall 5: RED-04 warning in mock mode
**What goes wrong:** Mock mode creates providers with literal `ApiKey::from("mock-test-key-cheap")` values. If the literal key warning fires for mock providers, it would be confusing.
**Why it happens:** Mock mode bypasses `from_file_with_env` and returns an empty `key_sources` vec, so the key source loop doesn't execute for mock providers.
**How to avoid:** The current code already handles this correctly -- mock mode returns `vec![]` for key_sources, so no warnings are emitted. Verify this doesn't change.
**Warning signs:** Users see plaintext key warnings when running with `--mock`.

### Pitfall 6: Permission check blocks Windows/macOS compilation
**What goes wrong:** `std::os::unix::fs::PermissionsExt` is only available on Unix targets. Code that imports it unconditionally fails to compile on Windows.
**Why it happens:** The trait is behind `#[cfg(unix)]` in the standard library.
**How to avoid:** Both the import and the function body must be guarded by `#[cfg(unix)]`. Provide a `#[cfg(not(unix))]` stub that returns `None`. Use `cfg(unix)` (not `cfg(target_os = "linux")`) to cover Linux, macOS, FreeBSD, etc.
**Warning signs:** Compilation error on macOS or Windows: `unresolved import std::os::unix`.

### Pitfall 7: Warning message format for permission octal
**What goes wrong:** Printing `mode` as decimal (e.g., `420` for `0o644`) instead of octal makes the warning confusing to users.
**Why it happens:** Default integer formatting in Rust uses decimal.
**How to avoid:** Use `format!("{:04o}", mode)` to print permissions in the familiar octal format (e.g., `0644`).
**Warning signs:** Warning says "permissions 420" instead of "permissions 0644".

## Code Examples

### Complete file permission check function

```rust
// Source: std::os::unix::fs::PermissionsExt (stdlib)
// Location: src/config.rs

/// Check if a config file has permissions more permissive than 0600.
///
/// Returns `Some((path_display, mode_octal))` if overly permissive, `None` if OK.
/// On non-Unix platforms, always returns `None`.
#[cfg(unix)]
pub fn check_file_permissions(path: &std::path::Path) -> Option<(String, u32)> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).ok()?;
    let mode = metadata.permissions().mode() & 0o777;

    // 0o600 = owner read+write only. Anything else set = too permissive.
    if mode & 0o177 != 0 {
        Some((path.display().to_string(), mode))
    } else {
        None
    }
}

#[cfg(not(unix))]
pub fn check_file_permissions(_path: &std::path::Path) -> Option<(String, u32)> {
    None
}
```

### Complete masked_prefix method

```rust
// Location: src/config.rs, impl ApiKey

impl ApiKey {
    /// Return a masked representation showing a recognizable prefix.
    ///
    /// Format: first N characters + "...***" where N = min(6, key_length).
    /// For keys shorter than 10 characters, returns "[REDACTED]" to avoid
    /// revealing a significant portion.
    ///
    /// Examples:
    /// - "cashuAbcdef..." -> "cashuA...***"
    /// - "sk-proj-abc..." -> "sk-pro...***"
    /// - "short" -> "[REDACTED]" (too short to mask safely)
    pub fn masked_prefix(&self) -> String {
        let secret = self.expose_secret();
        if secret.len() < 10 {
            return "[REDACTED]".to_string();
        }
        let prefix_len = 6;
        format!("{}...***", &secret[..prefix_len])
    }
}
```

### Startup permission and literal key warnings in serve command

```rust
// Location: src/main.rs, Commands::Serve arm (after config loading)

// RED-01: Warn if config file permissions are too open
if !mock {
    if let Some((path, mode)) = arbstr::config::check_file_permissions(std::path::Path::new(&config_path)) {
        tracing::warn!(
            file = %path,
            permissions = format_args!("{:04o}", mode),
            "Config file has permissions more open than 0600. \
             Consider: chmod 600 {}",
            path
        );
    }
}

// Key source logging with RED-04 literal key warning
for (provider_name, source) in &key_sources {
    match source {
        KeySource::Literal => {
            tracing::info!(provider = %provider_name, "key from config-literal");
            tracing::warn!(
                provider = %provider_name,
                "Plaintext API key in config file. \
                 Consider using environment variables: set {} or use api_key = \"${{{}}}\"",
                arbstr::config::convention_env_var_name(provider_name),
                arbstr::config::convention_env_var_name(provider_name)
            );
        }
        KeySource::EnvExpanded => {
            tracing::info!(provider = %provider_name, "key from env-expanded")
        }
        KeySource::Convention(var) => {
            tracing::info!(provider = %provider_name, env_var = %var, "key from convention")
        }
        KeySource::None => {
            tracing::warn!(provider = %provider_name, "no api key available")
        }
    }
}
```

### Updated /providers endpoint

```rust
// Location: src/proxy/handlers.rs, list_providers function

pub async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    let providers: Vec<serde_json::Value> = state
        .router
        .providers()
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "models": p.models,
                "input_rate_sats_per_1k": p.input_rate,
                "output_rate_sats_per_1k": p.output_rate,
                "base_fee_sats": p.base_fee,
                "api_key": match &p.api_key {
                    Some(key) => serde_json::Value::String(key.masked_prefix()),
                    None => serde_json::Value::Null,
                },
            })
        })
        .collect();

    Json(serde_json::json!({
        "providers": providers
    }))
}
```

### Updated providers CLI command

```rust
// Location: src/main.rs, Commands::Providers arm

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
    if let Some(ref api_key) = provider.api_key {
        println!("    Key: {}", api_key.masked_prefix());
    }
    println!();
}
```

### Test: masked prefix

```rust
#[test]
fn test_masked_prefix_normal_key() {
    let key = ApiKey::from("cashuAbcdefghijklmnop");
    assert_eq!(key.masked_prefix(), "cashuA...***");
}

#[test]
fn test_masked_prefix_short_key_redacted() {
    let key = ApiKey::from("short");
    assert_eq!(key.masked_prefix(), "[REDACTED]");
}

#[test]
fn test_masked_prefix_boundary_key() {
    let key = ApiKey::from("1234567890"); // exactly 10 chars
    assert_eq!(key.masked_prefix(), "123456...***");
}

#[test]
fn test_masked_prefix_does_not_expose_full_key() {
    let key = ApiKey::from("cashuABCD1234secrettoken");
    let masked = key.masked_prefix();
    assert!(masked.contains("cashuA"), "Should show prefix");
    assert!(!masked.contains("secrettoken"), "Must not contain secret part");
    assert!(masked.ends_with("...***"), "Should end with mask");
}
```

### Test: file permission check (Unix only)

```rust
#[cfg(unix)]
#[test]
fn test_check_permissions_too_open() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-config.toml");
    std::fs::write(&path, "[server]\nlisten = \"127.0.0.1:8080\"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let result = check_file_permissions(&path);
    assert!(result.is_some(), "0644 should trigger warning");
    let (_, mode) = result.unwrap();
    assert_eq!(mode, 0o644);
}

#[cfg(unix)]
#[test]
fn test_check_permissions_correct() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-config.toml");
    std::fs::write(&path, "[server]\nlisten = \"127.0.0.1:8080\"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let result = check_file_permissions(&path);
    assert!(result.is_none(), "0600 should not trigger warning");
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| No permission check on config files | Advisory warning on startup (this phase) | Phase 7 | Users alerted to insecure config file permissions |
| Full `[REDACTED]` everywhere | Masked prefix `cashuA...***` in display contexts | Phase 7 | Users can verify which key is loaded without seeing full key |
| Silent acceptance of literal keys | Warning recommending env vars | Phase 7 | Users guided toward better secret hygiene |

**Deprecated/outdated:**
- The `fs-mistrust` crate was considered but is designed for Tor's security model (full path-chain validation). It is overkill for a single file permission check.

## Open Questions

1. **What prefix length is ideal for Cashu tokens?**
   - What we know: Cashu tokens start with "cashuA" (version byte) followed by base64-encoded data. Tokens are typically 100+ characters.
   - What's unclear: Whether 6 characters is universally sufficient for identification across all Cashu token versions.
   - Recommendation: **Use 6 characters.** This matches "cashuA" which is the identifying prefix. The `...***` suffix makes it clear the rest is hidden. If future token formats need longer prefixes, the constant can be adjusted.

2. **Should the `check` command also emit the plaintext literal warning?**
   - What we know: The `check` command already reports key sources. Adding a warning for Literal sources would be consistent with the serve command.
   - What's unclear: Whether the check command's output format (println) should mirror the serve command's tracing output.
   - Recommendation: **Yes, add the warning to `check` too.** Use `println!` format like: `"  WARNING: Plaintext key for '{name}'. Consider: set {conv_var} or use api_key = \"${conv_var}\""`. This makes `check` a comprehensive config audit tool.

3. **Should permission check also cover 0o400 (read-only for owner)?**
   - What we know: 0o600 means owner read+write. 0o400 means owner read-only. Both are secure (no group/other access).
   - What's unclear: Whether a read-only config file (0o400) should be considered acceptable.
   - Recommendation: **Accept both 0o600 and 0o400.** The check should only warn when group or other bits are set. The mask `mode & 0o177 != 0` correctly handles this -- it allows 0o600 and 0o400 (and 0o200, though that's unusual) while flagging anything with group/other bits.

## Sources

### Primary (HIGH confidence)
- [std::os::unix::fs::PermissionsExt](https://doc.rust-lang.org/std/os/unix/fs/trait.PermissionsExt.html) - `mode()` returns `u32` with Unix permission bits
- [std::fs::Permissions](https://doc.rust-lang.org/std/fs/struct.Permissions.html) - Cross-platform permissions struct
- [Rust conditional compilation](https://doc.rust-lang.org/reference/conditional-compilation.html) - `#[cfg(unix)]` attribute syntax

### Codebase (HIGH confidence)
- `src/config.rs` - ApiKey type with expose_secret(), KeySource enum, convention_env_var_name(), from_file_with_env()
- `src/main.rs` - Serve/check/providers commands with key_sources iteration loop already in place
- `src/proxy/handlers.rs` - list_providers handler currently emitting `"[REDACTED]"` for api_key
- `Cargo.toml` - No new dependencies needed; tempfile already in dev-dependencies for tests

### Prior Phase Context (HIGH confidence)
- Phase 5 CONTEXT.md: "Full `[REDACTED]` replacement everywhere -- no partial masks or prefixes (Phase 7 adds masked prefixes later)"
- Phase 5 RESEARCH.md: ApiKey newtype pattern, expose_secret() grep-auditability
- Phase 6 VERIFICATION.md: KeySource::Literal already tracks plaintext keys, convention_env_var_name() is public
- Phase 6: Mock mode returns empty key_sources vec (no warnings for mock providers)

### Secondary (MEDIUM confidence)
- [fs-mistrust crate](https://docs.rs/fs-mistrust/latest/fs_mistrust/) - Evaluated as alternative for permission checking (too complex for this use case)
- [RustDesk config permission PR](https://github.com/rustdesk/rustdesk/pull/7983) - Real-world example of enforcing 0600 on config files in a Rust project

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - stdlib only, all APIs verified from official docs
- Architecture: HIGH - builds directly on Phase 5 and 6 infrastructure; all integration points identified in codebase
- Pitfalls: HIGH - based on direct code analysis and known Unix permission semantics

**Research date:** 2026-02-15
**Valid until:** 90 days (stdlib APIs are stable; project architecture is well-understood from prior phases)
