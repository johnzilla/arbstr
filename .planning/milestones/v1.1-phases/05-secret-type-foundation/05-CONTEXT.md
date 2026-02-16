# Phase 5: Secret Type Foundation - Context

**Gathered:** 2026-02-15
**Status:** Ready for planning

<domain>
## Phase Boundary

Migrate api_key from plaintext String to SecretString so Debug, Display, and tracing never expose key values. All existing behavior preserved, all existing tests pass. No new capabilities (env var expansion, masked prefixes, permission warnings belong in later phases).

</domain>

<decisions>
## Implementation Decisions

### Redaction format
- Full `[REDACTED]` replacement everywhere — no partial masks or prefixes (Phase 7 adds masked prefixes later)
- Same `[REDACTED]` string across all contexts: CLI output, JSON responses, debug/tracing logs
- SecretString Debug impl emits just `[REDACTED]` — surrounding struct's derive(Debug) handles field names
- Error messages reference provider name only, never any key-related info

### API key optionality
- api_key stays **required** in this phase — only the type changes from String to SecretString
- Phase 6 will make it optional for convention-based env var lookup
- TOML config stays exactly the same: `api_key = "cashuA..."` — serde deserializes transparently into SecretString
- Actual key value accessed via `.expose_secret()` — makes every access point explicit and grep-auditable
- Mock providers use SecretString too — consistent type system, tests verify redaction works

### JSON serialization
- `/providers` endpoint includes `"api_key": "[REDACTED]"` — field present but redacted, confirms key is configured
- No extra `has_key` boolean — redacted value presence is sufficient
- CLI `providers` command removes the key column entirely — a column of identical `[REDACTED]` adds no value
- Custom Serialize impl on the config type to always produce `[REDACTED]` — do not rely on secrecy's default Serialize which exposes the actual secret

### Claude's Discretion
- Wrapper type design (newtype around secrecy::SecretString vs direct use)
- How to structure the custom Serialize to avoid accidental exposure
- Test strategy for verifying redaction in each output surface

</decisions>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches. Research already identified secrecy v0.10.3 as the crate choice with serde support.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 05-secret-type-foundation*
*Context gathered: 2026-02-15*
