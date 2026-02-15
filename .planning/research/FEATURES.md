# Feature Landscape: Secrets Handling in CLI/Proxy Applications

**Domain:** Secrets management for CLI tools and local proxy servers
**Researched:** 2026-02-15
**Overall confidence:** HIGH (well-established patterns across mature tooling ecosystem)

## Competitive Landscape Summary

Secrets handling in CLI tools and proxies is a solved problem with clear conventions. LiteLLM, Docker Compose, Terraform, kubectl, and the broader cloud CLI ecosystem have converged on a layered approach: environment variables as the standard injection mechanism, config-file syntax for referencing them, and type-based redaction to prevent leaks. The specific patterns vary, but the hierarchy is universal: env vars > config file > hardcoded.

arbstr currently stores `api_key` as `Option<String>` in plaintext TOML, derives `Debug` on all config structs (exposing keys in log output), prints provider URLs in the `providers` CLI command, and logs `provider.url` in tracing spans. The `/providers` HTTP endpoint deliberately omits `api_key` from its JSON response (good), but `Debug` derives on `ProviderConfig` and `SelectedProvider` would expose keys if any code path logs those structs.

---

## Table Stakes

Features users expect. Missing any of these makes the tool a security liability or forces workarounds that defeat the purpose of config files.

| Feature | Why Expected | Complexity | Confidence | Notes |
|---------|--------------|------------|------------|-------|
| **Environment variable expansion in config values** | Every serious CLI tool supports this. Docker Compose uses `${VAR}`, LiteLLM uses `os.environ/VAR`, Terraform uses `var.name` with env override. Users should never have to put actual secrets in a file that might get committed. | Medium | HIGH | Use `${VAR}` or `${VAR:-default}` syntax. Parse during config loading, before validation. Fail with a clear error if the env var is not set and no default is provided. |
| **Convention-based env var lookup when api_key is omitted** | Terraform providers, AWS CLI, and cloud SDKs all auto-discover credentials from well-known env vars. For arbstr, if a provider named "routstr" has no `api_key` in config, check `ARBSTR_ROUTSTR_API_KEY` automatically. Reduces config boilerplate and keeps secrets out of files entirely. | Low | HIGH | Naming convention: `ARBSTR_<UPPER_SNAKE_NAME>_API_KEY`. Fallback after explicit config and env expansion both miss. |
| **Redact keys from Debug output** | The `secrecy` crate exists specifically for this. Rust's `#[derive(Debug)]` on structs containing secrets will print them when the struct is logged, debug-printed, or appears in error messages. Every security-conscious Rust project wraps secret fields. | Low | HIGH | Replace `api_key: Option<String>` with `api_key: Option<SecretString>` using the `secrecy` crate. `Debug` impl prints `[REDACTED]` instead of the actual value. |
| **Redact keys from tracing/log output** | tracing structured fields will format any `Debug`-implementing type. If `ProviderConfig` or `SelectedProvider` is ever logged at debug level (common during development), keys leak to stderr/files. Current code logs `provider.url` in an info span -- not a key leak itself, but `Debug` on the parent struct is. | Low | HIGH | Handled automatically once `SecretString` replaces `String` for key fields, since `SecretString` implements `Debug` as `[REDACTED]`. |
| **Redact keys from API responses** | The `/providers` HTTP endpoint already omits `api_key` from its JSON output (lines 766-779 of handlers.rs construct the response with only name, models, and rates). Verify this holds. The `error_body` in error messages (line 518-528) could contain provider responses that echo back auth headers -- these should be scrubbed. | Low | HIGH | Audit all JSON serialization paths. The current `/providers` endpoint is safe. Error paths that forward provider response bodies need scrubbing. |
| **Redact keys from CLI output** | The `providers` CLI command (main.rs lines 120-133) currently prints `provider.name` and `provider.url` but not `api_key`. However, if someone adds a `--verbose` flag or uses `{:?}` formatting on the config struct, keys would leak. Making the type itself safe prevents future regressions. | Low | HIGH | Covered by the `SecretString` type change. Also add an explicit "has key: yes/no" indicator to CLI output so users can verify config without seeing the key. |
| **Config file permission warning** | SSH, gpg, and SSSD all refuse to operate or warn when secret-containing files have world-readable permissions. A config file with `api_key = "cashuA..."` at mode 0644 is a local security issue. | Low | HIGH | Check `stat()` on config file at load time. Warn (do not hard-fail) if permissions are more permissive than 0600. Unix-only; skip on Windows. |

### Implementation Priority for Table Stakes

1. **SecretString type change** -- Foundation. Changes `api_key` field type, which every other feature depends on. Do this first because it changes the public API of config structs.
2. **Environment variable expansion** -- Core config-loading change. Implement the `${VAR}` parser in `Config::parse_str` or a pre-processing step before TOML parsing.
3. **Convention-based env var lookup** -- Small addition after expansion works. Check `ARBSTR_<NAME>_API_KEY` when `api_key` is None after TOML parsing and expansion.
4. **Config file permission warning** -- Quick `stat()` check in `Config::from_file`.
5. **Audit output surfaces** -- Verify no regression in `/providers`, `providers` CLI, error messages, and tracing output.

---

## Differentiators

Features that go beyond what users strictly expect but signal a well-designed tool. Not common in simple proxy tools, but present in security-focused infrastructure.

| Feature | Value Proposition | Complexity | Confidence | Notes |
|---------|-------------------|------------|------------|-------|
| **Startup validation with key source reporting** | On `serve` startup, log which providers have keys and where each key came from: "provider-alpha: key from config (env expanded)", "provider-beta: key from ARBSTR_PROVIDER_BETA_API_KEY", "provider-gamma: no key configured". Gives immediate visibility without showing the key. | Low | HIGH | LiteLLM logs model configuration at startup. arbstr already logs provider count. Adding source info is trivial and extremely helpful for debugging "why is my provider returning 401?". |
| **`check` command validates key availability** | The existing `check` command validates config structure. Extend it to verify that env vars referenced by `${VAR}` are set and that convention-based env vars exist for keyless providers. Report clearly: "provider-alpha: key will resolve from $ROUTSTR_KEY (currently set)" or "provider-beta: no key source found (will be unauthenticated)". | Low | HIGH | Catches config mistakes at validate-time rather than first-request-time. Every good CLI tool does this (terraform validate, docker compose config). |
| **Key masking in "providers" output** | Instead of omitting the key entirely, show `api_key: cashuA...***` (first 6 chars + mask) in both CLI and HTTP output. Lets users verify which key is loaded without exposing the full value. | Low | MEDIUM | Common in cloud dashboards (AWS console shows last 4 of access key). Useful for multi-provider configs where you need to confirm the right key is on the right provider. |
| **Zeroize on drop for secret values** | The `secrecy` crate's `SecretString` uses `zeroize` to clear secret memory when the value is dropped. Prevents secrets from lingering in freed memory pages that could be read by a debugger or core dump. | Free | HIGH | This comes automatically with `SecretString` from the `secrecy` crate. No extra work -- it is the default behavior. Mention in docs as a security property. |
| **Warn on plaintext key in config when env expansion available** | If a user writes `api_key = "cashuA..."` (literal value, no `${}`), emit a startup warning suggesting they use an env var instead. Educates users toward the secure path without blocking the insecure one. | Low | MEDIUM | Gentle nudge, not enforcement. Detected by checking if the resolved value matches the raw TOML value and contains no `${}` syntax. |

### Differentiator Priority

1. **Startup validation with key source reporting** -- Free win during implementation. Log it as part of the config loading path.
2. **`check` command key validation** -- Natural extension of existing `check` command.
3. **Key masking in output** -- Small utility, nice polish.
4. **Warn on plaintext key** -- Controversial (could annoy users), implement last and consider making it suppressible.
5. **Zeroize on drop** -- Free with SecretString. Just document it.

---

## Anti-Features

Features to deliberately NOT build in this milestone. Over-engineering secrets management for a local single-user proxy is a real risk.

| Anti-Feature | Why Other Products Have It | Why arbstr Should NOT Build It | What to Do Instead |
|--------------|---------------------------|-------------------------------|--------------------|
| **External secrets manager integration (Vault, AWS SM, Doppler)** | LiteLLM supports Google KMS and Azure KMS. Enterprise tools need centralized secret rotation and audit trails. | arbstr runs on a home network for one user. Adding vault client dependencies, authentication flows, and network calls to fetch a single API key is massive over-engineering. The user's threat model is "don't commit the key to git," not "SOC2 compliance." | Environment variables are the universal interface. If the user wants Vault, they use `vault exec` or `direnv` to inject env vars before running arbstr. arbstr reads `${}` in config; how those vars get set is the user's concern. |
| **Encrypted config file** | Some tools encrypt secrets at rest in config files (ansible-vault, sops). | Adds a key-management-for-the-key problem. The user now needs a master password or GPG key to decrypt the config, which must itself be stored somewhere. For a local proxy, file permissions (0600) + env vars achieve the same goal without the complexity. | Use env vars. Recommend `chmod 600 config.toml` in docs if users insist on putting keys in the file. |
| **Secret rotation / auto-refresh** | Cloud SDKs refresh temporary credentials automatically. LiteLLM supports key rotation. | Routstr/Cashu tokens are pre-funded and expire when balance hits zero, not on a time schedule. There is no "refresh" operation -- you fund a new session and get a new key. Rotation logic has no semantics to implement. | When a key expires (provider returns 401), the user creates a new Cashu token and updates the env var. Restart arbstr or (future) support config reload. |
| **Keyring / OS credential store integration** | macOS Keychain, Windows Credential Manager, Linux secret-service. Some CLI tools (gh, aws-vault) store tokens in the OS keyring. | Adds `keyring` crate dependency with platform-specific backends (libsecret on Linux, Security.framework on macOS). Significantly increases build complexity and platform-specific testing burden for marginal benefit over env vars. | Skip. Env vars work cross-platform. If the user wants keyring, they can use a wrapper like `secret-tool lookup` piped into an env var. |
| **Runtime secret injection via API** | Some proxies accept key updates via admin API without restart. | Adds mutable state, authentication for the admin endpoint, and race conditions with in-flight requests. For a local tool, restarting is fine -- it takes <1 second. | Skip. Config changes require restart. (Future: SIGHUP config reload is simpler than an API.) |
| **Audit logging of secret access** | Enterprise tools log every time a secret is read, by whom, from where. | Single user. The audit trail is: "I ran arbstr." There is no unauthorized access to log. | Skip entirely. |

---

## Feature Dependencies

```
SecretString type change (api_key: Option<SecretString>)
  |
  +-- Debug redaction (automatic via SecretString)
  |     |
  |     +-- Tracing/log redaction (automatic via SecretString Debug impl)
  |     |
  |     +-- CLI output safety (Debug prints [REDACTED])
  |
  +-- Zeroize on drop (automatic via SecretString)
  |
  +-- Serde Deserialize support (secrecy "serde" feature)
        |
        +-- Config file still parses normally via toml + serde

Environment variable expansion (pre-processing step)
  |
  +-- Config parse pipeline change (expand before or during TOML parse)
  |
  +-- Convention-based lookup (check ARBSTR_<NAME>_API_KEY after expansion)
  |
  +-- Startup key source reporting (log where each key came from)
  |
  +-- `check` command key validation (verify env vars resolve)

Config file permission check (independent)
  |
  +-- Warning on load (stat() call in Config::from_file)

Output surface audit (depends on SecretString type change)
  |
  +-- /providers endpoint (already safe, verify no regression)
  |
  +-- Error message scrubbing (provider error bodies may echo auth)
  |
  +-- Key masking for display (first N chars + ***)
```

Key dependency insight: **The SecretString type change is the foundation.** It must happen first because it changes the `ProviderConfig` struct's public API. Everything downstream (debug safety, serde compat, display masking) builds on having the right type. Environment variable expansion is independent and can be developed in parallel, but must be integrated with SecretString deserialization (expand the string, then wrap in SecretString).

---

## MVP Recommendation

Based on the analysis above and arbstr's specific context (single-user local proxy, Cashu tokens as API keys, existing plaintext TOML config), here is the recommended feature set for the v1.1 Secrets Hardening milestone.

### Must Have (Table Stakes)

1. **SecretString for api_key field** -- Replace `Option<String>` with `Option<SecretString>` using the `secrecy` crate with serde feature. This is the single highest-leverage change: it fixes Debug, tracing, and accidental logging in one shot.
2. **Environment variable expansion** -- Support `${VAR_NAME}` syntax in TOML string values. At minimum for `api_key`, ideally for any string field (url could also benefit). Fail with clear error when env var is unset.
3. **Convention-based env var lookup** -- When `api_key` is omitted from a provider config, automatically check `ARBSTR_<UPPER_SNAKE_PROVIDER_NAME>_API_KEY`. This is the zero-config secure path.
4. **Config file permission warning** -- Warn on startup if config file is group/world readable and contains any api_key values.
5. **Output surface audit** -- Verify `/providers` endpoint, `providers` CLI command, error messages, and tracing output do not leak keys after SecretString migration.

### Should Have (Differentiators worth the effort)

6. **Startup key source reporting** -- Log per-provider key source (config/env-expanded/convention/none) at info level during startup.
7. **`check` command key validation** -- Extend existing check to verify env var resolution for all providers.
8. **Key masking for display** -- Show `cashuA...***` in providers CLI output so users can verify key identity.

### Defer

9. **Warn on plaintext key** -- Nice in theory but could annoy experienced users. Evaluate after core features land.
10. **External secrets manager integration** -- Wrong scope for a local tool.

---

## Complexity Estimates

| Feature | Lines of Code (est.) | New Files | Touches Existing | Risk |
|---------|---------------------|-----------|-----------------|------|
| SecretString migration | ~60 | None | config.rs, selector.rs, handlers.rs | Low -- type change + expose_secret() at use sites |
| Env var expansion | ~80 | None (or config/expand.rs) | config.rs (parse pipeline) | Medium -- regex/parser for ${VAR} syntax, error handling |
| Convention-based lookup | ~40 | None | config.rs (post-parse step) | Low -- env::var() with name transform |
| File permission check | ~25 | None | config.rs (from_file method) | Low -- stat() + mode check |
| Output surface audit | ~30 | None | handlers.rs, main.rs | Low -- verification + minor adjustments |
| Startup key source reporting | ~30 | None | server.rs or main.rs | Low -- log statements |
| Check command key validation | ~40 | None | main.rs | Low -- extend existing path |
| Key masking display | ~20 | None | main.rs, optionally handlers.rs | Low -- string truncation |

**Total estimated new/changed code:** ~325 lines

---

## Current Leak Surface Inventory

Specific locations in the codebase where secrets could leak, informing the audit scope:

| Surface | File | Current State | Risk | Fix |
|---------|------|--------------|------|-----|
| `#[derive(Debug)]` on `ProviderConfig` | config.rs:52 | Exposes `api_key` field in any debug log | HIGH | SecretString |
| `#[derive(Debug)]` on `SelectedProvider` | selector.rs:9 | Exposes `api_key` field | HIGH | SecretString |
| `#[derive(Debug)]` on `Router` | selector.rs:33 | Contains `Vec<ProviderConfig>`, cascades Debug | HIGH | SecretString fixes it transitively |
| `tracing::info!(url = %provider.url, ...)` | handlers.rs:577-578 | Logs provider URL (not the key itself) | LOW | URL is not a secret, but verify URL never contains key as query param |
| `format!("Bearer {}", api_key)` | handlers.rs:500 | Constructs auth header -- correct behavior, not logged | NONE | No change needed |
| Provider error body forwarded | handlers.rs:518-528 | Provider error response echoed in logs and to client | MEDIUM | Scrub any auth-related content from error bodies |
| `/providers` JSON response | handlers.rs:766-779 | Deliberately omits api_key -- safe | NONE | Verify after type change |
| `providers` CLI command | main.rs:120-133 | Prints name and URL, not key -- safe | LOW | Add "has key: yes/no" indicator |
| `Config` Debug derive | config.rs:7 | Root struct, cascades to all children including keys | HIGH | SecretString on leaf field fixes cascade |

---

## Sources and Confidence Notes

- **secrecy crate** ([docs.rs/secrecy](https://docs.rs/secrecy/latest/secrecy/), [crates.io](https://crates.io/crates/secrecy)): HIGH confidence. Well-maintained, widely used in Rust ecosystem. Provides SecretString with Debug redaction and zeroize-on-drop. Supports serde Deserialize via feature flag.
- **LiteLLM env var expansion** ([docs.litellm.ai/docs/proxy/configs](https://docs.litellm.ai/docs/proxy/configs)): HIGH confidence. Uses `os.environ/VAR` syntax in YAML config. Verified pattern of config-level env var injection in LLM proxies.
- **Docker Compose variable interpolation** ([Docker Docs](https://docs.docker.com/compose/how-tos/environment-variables/variable-interpolation/)): HIGH confidence. Uses `${VAR}` and `${VAR:-default}` syntax. Industry-standard convention.
- **OWASP Secrets Management Cheat Sheet** ([cheatsheetseries.owasp.org](https://cheatsheetseries.owasp.org/cheatsheets/Secrets_Management_Cheat_Sheet.html)): HIGH confidence. Authoritative guidance on secrets handling.
- **File permission conventions** (SSH, SSSD, GPG): HIGH confidence. Well-established Unix convention of refusing or warning when secret files are world-readable.
- **redaction library** ([github.com/sformisano/redaction](https://github.com/sformisano/redaction)): MEDIUM confidence. Newer crate, less battle-tested than secrecy. Noted as alternative but secrecy is the established choice.
- **Convention-based env var naming**: MEDIUM confidence. Pattern derived from AWS CLI (`AWS_ACCESS_KEY_ID`), Terraform provider env vars, and general cloud SDK conventions. The specific `ARBSTR_<NAME>_API_KEY` convention is arbstr-specific but follows established patterns.
