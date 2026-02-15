# Project Research Summary

**Project:** arbstr v1.1 - Secrets Hardening
**Domain:** Secrets management and configuration hardening for Rust LLM proxy
**Researched:** 2026-02-15
**Confidence:** HIGH

## Executive Summary

The v1.1 Secrets Hardening milestone adds environment variable expansion and API key redaction to arbstr, a Rust-based intelligent LLM routing proxy. The research reveals that secrets handling in Rust CLI tools and proxies is a well-solved problem with established patterns: the `secrecy` crate provides type-driven redaction and zeroization, environment variable expansion follows Docker Compose's `${VAR}` convention, and convention-based env var lookup (like `ARBSTR_<PROVIDER>_API_KEY`) matches cloud SDK patterns.

The recommended approach is a two-phase config loading pipeline: deserialize TOML into internal structs with plain `String` fields, then expand env vars and wrap secrets in `SecretString` before validation. This provides excellent error messages (with provider context when vars are missing), natural integration with convention-based fallback, and type-driven protection against accidental exposure via Debug formatting or serialization. The stack addition is minimal—just the `secrecy` crate with serde support—while the unused `config` crate can be removed, resulting in net-zero dependency growth.

The critical risk is that arbstr's current codebase derives `Debug` on all config structs containing API keys, meaning any panic, debug log, or test assertion failure currently leaks secrets in plaintext. The SecretString type migration addresses this automatically through Rust's type system, but requires careful sequencing: type changes first (foundation), then env var expansion (configuration), then output surface audit (hardening). All research sources are official docs and established Rust ecosystem standards, giving this milestone HIGH confidence for execution.

## Key Findings

### Recommended Stack

The existing arbstr stack requires only one addition and one removal for secrets handling. The `secrecy` crate (v0.10.3) is the de facto Rust ecosystem standard for wrapping secrets, providing `SecretString` with automatic Debug redaction (`[REDACTED]`), memory zeroing on drop via `zeroize`, and optional serde support. The unused `config` crate (v0.14) should be removed—it's declared in Cargo.toml but never imported, and the project's current TOML-based config loading works well.

Environment variable expansion requires no new dependencies—a ~30-line pure function using `std::env::var` handles the `${VAR}` pattern. Available crates (shellexpand, serde-env-field, serde-with-expand-env) bring unwanted complexity or extraneous features for this focused use case.

**Core technologies:**
- **secrecy v0.10 (serde feature)**: Wraps API keys in `SecretString` for automatic redaction and zeroize-on-drop—ecosystem standard, minimal API surface
- **std::env + manual expansion**: 30-line pure function for `${VAR}` expansion—no crate needed, trivially testable, fail-fast on missing vars
- **Remove: config crate**: Unused dependency—project loads config via `toml::from_str`, no code references this crate

For full details, see [STACK.md](./STACK.md).

### Expected Features

Secrets handling for CLI tools and proxies has clear table stakes (env var expansion, Debug redaction, convention-based lookup) and useful differentiators (startup key source reporting, check command validation). The competitive analysis of LiteLLM, Docker Compose, Terraform, and kubectl confirms these patterns are universal expectations.

**Must have (table stakes):**
- **SecretString for api_key field**: Users expect Debug output to not leak credentials—basic Rust security hygiene
- **Environment variable expansion (${VAR})**: Every serious CLI tool supports this—Docker Compose, LiteLLM, Terraform all use similar syntax
- **Convention-based env var lookup**: Cloud SDKs auto-discover credentials from well-known env vars (e.g., `ARBSTR_<PROVIDER>_API_KEY`)
- **Redact keys from tracing/logs**: Structured logging means Debug formatting reaches stderr—automatic via SecretString type
- **Config file permission warning**: SSH, gpg, and SSSD all warn when secret files are world-readable

**Should have (competitive):**
- **Startup key source reporting**: Log which providers have keys and where each came from (config/env-expanded/convention/none)
- **check command key validation**: Verify env vars referenced by ${VAR} are set—catches config mistakes pre-runtime
- **Key masking in CLI output**: Show `cashuA...***` instead of omitting entirely—lets users verify key identity

**Defer (v2+):**
- **External secrets manager integration**: Wrong scope for local single-user proxy—env vars work with any secret manager via shell
- **Encrypted config file**: File permissions + env vars achieve same goal without key-for-key problem
- **Keyring/credential store**: Platform-specific complexity for marginal benefit over env vars

For full feature analysis including anti-features and complexity estimates, see [FEATURES.md](./FEATURES.md).

### Architecture Approach

The architecture uses a two-phase config loading pipeline to separate TOML parsing from environment resolution. Phase 1 deserializes into internal `RawProviderConfig` structs with plain `Option<String>` fields. Phase 2 expands `${VAR}` patterns, falls back to convention-based `ARBSTR_<NAME>_API_KEY` lookup when explicit keys are missing, and wraps the result in `Option<SecretString>` for the public `ProviderConfig`. This approach provides superior error messages, natural precedence ordering (explicit > expanded > convention), and compiler-driven migration.

**Major components:**
1. **RawProviderConfig (internal)**: Serde deserialization target with plain String fields—receives raw TOML, passes through to conversion
2. **expand_env_vars (pure function)**: Detects `${VAR_NAME}` patterns and expands via std::env::var—fails fast on missing vars with clear errors
3. **SecretString wrapper**: Changes `api_key` type from String to SecretString—provides automatic Debug redaction and prevents accidental Serialize
4. **Modified config pipeline**: `TOML -> Raw -> expand -> convention fallback -> Secret-wrapped -> validate`

**Key patterns:**
- Type-driven redaction via SecretString—compiler enforces safety at Debug/Display/Serialize call sites
- Single expose_secret() call site—only handlers.rs Authorization header construction sees plaintext
- Precedence: explicit TOML > ${VAR} expansion > ARBSTR_<NAME>_API_KEY convention > no key
- Error messages include provider context when env vars are missing

For detailed implementation guidance, component diagrams, and build order, see [ARCHITECTURE.md](./ARCHITECTURE.md).

### Critical Pitfalls

The research identified 11 domain pitfalls, with 4 rated CRITICAL severity. All relate to the current codebase's Debug derives on secret-bearing structs and the subtleties of environment variable resolution.

1. **Debug derive on config structs exposes API keys in logs**: `ProviderConfig`, `SelectedProvider`, `Router`, `Config`, and `AppState` all derive Debug with api_key as plain String—any panic/debug log/test assertion leaks keys. Fix: Change to `SecretString` (automatic redaction via its Debug impl).

2. **Error messages include provider context that leaks keys**: handlers.rs forwards reqwest errors (may contain URLs with embedded credentials) and provider error bodies (may echo back "Invalid API key: cashuA...") directly to clients. Fix: Separate internal log messages from client-facing errors.

3. **Env var expansion that fails open (missing var = empty string)**: Common mistake is `unwrap_or_default()` which silently treats missing vars as empty—leads to confusing 401 errors. Fix: Treat missing vars as hard errors with provider context.

4. **Convention-based lookup conflicts with explicit config**: Without clear precedence, overlapping explicit `api_key = "value"` and `ARBSTR_PROVIDER_API_KEY` env var creates unpredictable behavior. Fix: Document and enforce precedence, log which source was used.

5. **tracing::info! logs provider URLs (which may contain credentials)**: The execute_request function logs provider.url at info level—if URLs contain embedded credentials, they leak. Fix: Remove url from info-level logs or redact it.

For all 11 pitfalls with specific code locations, prevention strategies, and phase assignments, see [PITFALLS.md](./PITFALLS.md).

## Implications for Roadmap

Based on research, the milestone should be structured in three sequential phases with clear dependency ordering. The SecretString type migration is the foundation—all other work builds on having the right type. Environment variable expansion is independent config loading work that must integrate with SecretString. Output surface audit is hardening that catches what the type system cannot enforce.

### Phase 1: SecretString Type Migration
**Rationale:** Foundation phase. Changes `api_key: Option<String>` to `Option<SecretString>` across all config and routing code. This is a mechanical type migration where the compiler identifies every required change. Must come first because it changes the public API of `ProviderConfig`, which every subsequent feature depends on.

**Delivers:**
- Type-safe secret wrapping with automatic Debug redaction
- Zeroize-on-drop for all API key copies
- Compile-time prevention of accidental Serialize
- Single expose_secret() call site in production code

**Addresses:**
- Must-have: SecretString for api_key field (table stakes)
- Must-have: Redact keys from Debug/tracing output (table stakes)

**Avoids:**
- Pitfall 1: Debug derive leaking keys (CRITICAL)
- Pitfall 2: Clone creating untracked copies
- Pitfall 3: /providers serialization risk

**Estimated effort:** ~60 LOC changes across config.rs, selector.rs, handlers.rs

### Phase 2: Environment Variable Expansion
**Rationale:** Configuration phase. Implements `${VAR}` syntax and convention-based `ARBSTR_<NAME>_API_KEY` lookup. Depends on Phase 1 because expanded values must produce SecretString, not String. The two-phase config loading pattern (Raw -> expand -> Secret-wrapped) provides superior error messages and natural precedence handling.

**Delivers:**
- `${VAR}` expansion with fail-fast on missing vars
- Convention-based env var auto-discovery
- Clear precedence: explicit > expanded > convention > none
- Provider-context error messages ("Provider 'alpha': env var 'KEY' not set")

**Addresses:**
- Must-have: Environment variable expansion (table stakes)
- Must-have: Convention-based env var lookup (table stakes)
- Should-have: check command key validation

**Avoids:**
- Pitfall 6: Failing open on missing env vars
- Pitfall 7: Convention/explicit precedence conflicts
- Pitfall 9: Provider name normalization edge cases

**Uses:**
- std::env (no new dependencies for expansion)
- Two-phase deserialization (Raw struct -> conversion -> Public struct)

**Estimated effort:** ~80 LOC for expansion logic + ~40 LOC for convention fallback

### Phase 3: Output Surface Audit
**Rationale:** Hardening phase. Reviews all code paths where secrets or secret-adjacent data might reach external surfaces (HTTP error responses, tracing calls, endpoint JSON). Depends on Phases 1-2 because the type migration eliminates the most dangerous leak paths; this phase catches edge cases the type system cannot enforce.

**Delivers:**
- Error messages that never include provider error bodies or reqwest errors
- Tracing audit (remove provider.url from info-level logs)
- /providers endpoint verification (no regression in manual JSON construction)
- Startup key source reporting (log where each key came from)
- Config file permission warning (Unix only)

**Addresses:**
- Must-have: Config file permission warning (table stakes)
- Should-have: Startup key source reporting (differentiator)
- Should-have: Key masking in CLI output (differentiator)

**Avoids:**
- Pitfall 4: Error messages leaking keys (CRITICAL)
- Pitfall 5: tracing logging URLs with credentials

**Implements:**
- Separate internal (logged) vs external (returned to client) error messages
- Key source enum: ConfigLiteral | ConfigExpanded | ConventionEnv | None
- File permission check via stat() with mode validation

**Estimated effort:** ~30 LOC for error scrubbing + ~30 LOC for key source reporting + ~25 LOC for permission check

### Phase Ordering Rationale

- **Type changes before env var logic**: Expanding into String then wrapping in SecretString is awkward and error-prone. Expanding into SecretString directly (Phase 1 complete, Phase 2 builds on it) provides clean integration and better error handling.

- **Foundation before configuration before hardening**: Phase 1 makes secrets safe by default via the type system. Phase 2 adds user-facing config features. Phase 3 audits remaining leak surfaces that types can't catch (error messages, logs). Each layer depends on the previous.

- **Compiler-driven migration**: Phase 1 intentionally breaks compilation everywhere api_key is used. The compiler errors are a checklist—you cannot miss a call site. This is Rust's type system working as intended.

- **No parallelization needed**: All three phases are small (60-80 LOC each per estimates). Sequential execution with clear handoffs is simpler than parallel work with integration risk.

- **Architecture supports incremental deployment**: Each phase delivers value independently. Phase 1 can ship (redaction only), then Phase 2 (env vars), then Phase 3 (hardening). This is not just a planning artifact—the phases can be deployed separately if needed.

### Research Flags

**Phases with standard patterns (skip research-phase):**
- **Phase 1 (SecretString)**: Secrecy crate is well-documented on docs.rs, type migration is mechanical, compiler drives completeness. No additional research needed.
- **Phase 2 (Env expansion)**: Stdlib work only, pattern documented in ARCHITECTURE.md, pure function is trivially testable. No additional research needed.
- **Phase 3 (Output audit)**: Code review and security best practices, not new patterns. Error handling separation is standard. No additional research needed.

**No phases need deeper research.** All patterns are established, sources are official docs, implementation is straightforward Rust. The research is comprehensive and actionable.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Secrecy v0.10.3 verified on crates.io/docs.rs, serde feature confirmed. Env expansion requires no crates. Existing config crate confirmed unused via code analysis. |
| Features | HIGH | Table stakes verified against LiteLLM, Docker Compose, Terraform, kubectl docs. Anti-features analysis matches arbstr's single-user local proxy use case. Feature priorities clear. |
| Architecture | HIGH | Two-phase deserialization is standard Rust pattern. Secrecy integration documented with examples. Direct codebase analysis confirms all api_key flow paths (config -> router -> handlers). |
| Pitfalls | HIGH | All critical pitfalls directly observable in current code (Debug derives at config.rs:52, selector.rs:9, error forwarding at handlers.rs:506-508/526-529). Prevention strategies verified against secrecy docs. |

**Overall confidence:** HIGH

Research is based on official documentation (secrecy crate, stdlib), established ecosystem patterns (Docker Compose env vars, cloud SDK conventions), and direct analysis of the arbstr codebase. All recommendations are actionable with clear implementation paths. No areas of uncertainty require additional research.

### Gaps to Address

**None identified.**

The research covers all aspects of the v1.1 milestone:
- Stack changes are minimal and verified (add secrecy, remove config)
- Feature scope is well-defined with clear table stakes vs differentiators
- Architecture provides concrete implementation guidance (two-phase loading, type-driven redaction)
- Pitfalls are directly mapped to current codebase with specific prevention strategies and line numbers

**Minor design decisions to finalize during planning:**
- Provider name normalization convention for env var names (recommend: uppercase, replace non-alphanumeric with underscore)
- Whether to support `${VAR:-default}` syntax (recommend: NO for v1.1, add in v1.2 if needed)
- Which string fields support env var expansion (recommend: `api_key` and `url` only)

These are implementation details, not research gaps. The core patterns and approaches are fully validated.

## Sources

### Primary (HIGH confidence)
- [secrecy crate v0.10.3 on crates.io](https://crates.io/crates/secrecy) — SecretString, serde feature, version verification
- [secrecy API documentation on docs.rs](https://docs.rs/secrecy/latest/secrecy/) — SecretString type alias, ExposeSecret trait, Debug behavior, Deserialize impl
- [secrecy GitHub (iqlusioninc/crates)](https://github.com/iqlusioninc/crates/tree/main/secrecy) — source code, zeroize integration, intentional lack of Serialize
- [Docker Compose variable interpolation docs](https://docs.docker.com/compose/how-tos/environment-variables/variable-interpolation/) — ${VAR} syntax convention
- Direct codebase analysis of `/home/john/vault/projects/github.com/arbstr/src/` — all source files analyzed for api_key flow and leak surfaces

### Secondary (MEDIUM confidence)
- [Secure Configuration and Secrets Management in Rust with Secrecy](https://leapcell.io/blog/secure-configuration-and-secrets-management-in-rust-with-secrecy-and-environment-variables) — practical integration patterns
- [Rust users forum: secrecy Serialize discussion](https://users.rust-lang.org/t/secrecy-crate-serialize-string/112263) — confirms no Serialize by default is intentional design
- [Rust zeroize move/copy pitfalls](https://benma.github.io/2020/10/16/rust-zeroize-move.html) — memory copies surviving moves/drops
- LiteLLM proxy configuration docs — env var patterns for LLM proxies
- Terraform provider env var conventions — convention-based credential lookup patterns

### Tertiary (LOW confidence)
- shellexpand, serde-env-field, serde-with-expand-env crates — evaluated and rejected alternatives
- redact, redaction crates — alternative approaches, evaluated and rejected (secrecy is ecosystem standard)
- veil crate — derive-based redaction, noted as alternative approach but secrecy is more widely adopted

---
*Research completed: 2026-02-15*
*Ready for roadmap: yes*
