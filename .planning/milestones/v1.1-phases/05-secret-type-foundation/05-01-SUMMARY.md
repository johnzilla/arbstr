---
phase: 05-secret-type-foundation
plan: 01
subsystem: auth
tags: [secrecy, secret-string, zeroize, api-key, redaction]

# Dependency graph
requires:
  - phase: 03-proxy-core
    provides: "ProviderConfig with api_key: Option<String>, proxy handlers, router selector"
provides:
  - "ApiKey newtype wrapping SecretString with redacted Debug/Display/Serialize"
  - "Zeroize-on-drop for all API key values in memory"
  - "expose_secret() as auditable single-point-of-access for raw key values"
  - "secrecy crate dependency"
affects: [06-env-var-expansion, 07-config-file-permissions]

# Tech tracking
tech-stack:
  added: [secrecy 0.10]
  removed: [config 0.14]
  patterns: [newtype-secret-wrapper, expose-secret-audit-trail]

key-files:
  created: []
  modified:
    - Cargo.toml
    - src/config.rs
    - src/router/selector.rs
    - src/proxy/handlers.rs
    - src/main.rs

key-decisions:
  - "ApiKey wraps SecretString directly (no intermediate trait) for simplicity"
  - "Custom Deserialize impl uses String::deserialize then wraps, avoiding SecretString::deserialize complexity"
  - "Mock providers use real ApiKey values (not None) to exercise key handling in tests"
  - "CLI providers command unchanged -- column of [REDACTED] adds no value"
  - "Removed unused config crate dependency"

patterns-established:
  - "ApiKey newtype: all secret values use dedicated wrapper types, never raw String"
  - "expose_secret() audit trail: grep for expose_secret to find all raw value access points"
  - "Redaction by default: Debug/Display/Serialize all produce [REDACTED]"

# Metrics
duration: 3min
completed: 2026-02-15
---

# Phase 5 Plan 1: Secret Type Foundation Summary

**ApiKey newtype wrapping secrecy::SecretString with redacted Debug/Display/Serialize, zeroize-on-drop, and single expose_secret() call site for Authorization header**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-15T21:52:52Z
- **Completed:** 2026-02-15T21:56:03Z
- **Tasks:** 2
- **Files modified:** 6

## Accomplishments
- Defined ApiKey newtype with custom Debug ("[REDACTED]"), Display ("[REDACTED]"), Serialize ("[REDACTED]"), delegated Deserialize, Clone, and zeroize-on-drop via SecretString
- Propagated Option<ApiKey> through ProviderConfig, SelectedProvider, proxy handlers, and mock config
- Single expose_secret() call site (Authorization header) -- all other output surfaces show [REDACTED]
- Added /providers JSON api_key field showing "[REDACTED]" or null
- Added 8 comprehensive redaction tests covering all output surfaces
- Removed unused config crate, added secrecy crate

## Task Commits

Each task was committed atomically:

1. **Task 1: Define ApiKey type and propagate through all layers** - `2764ded` (feat)
2. **Task 2: Add redaction tests and verify no regressions** - `a0323ee` (test)

## Files Created/Modified
- `Cargo.toml` - Added secrecy 0.10, removed unused config 0.14
- `src/config.rs` - Defined ApiKey newtype with all trait impls, changed ProviderConfig.api_key to Option<ApiKey>, added 8 redaction tests
- `src/router/selector.rs` - Changed SelectedProvider.api_key to Option<ApiKey>, added ApiKey import
- `src/proxy/handlers.rs` - Used expose_secret() for Authorization header, added redacted api_key to /providers JSON
- `src/main.rs` - Mock providers use Some(ApiKey::from("mock-test-key-..."))

## Decisions Made
- ApiKey wraps SecretString directly for simplicity -- no intermediate trait needed
- Custom Deserialize impl deserializes as String then wraps, avoiding SecretString serde complexity
- Mock providers use real ApiKey values to exercise the full key handling path
- CLI providers command left unchanged (no key column -- [REDACTED] column adds no value)
- Removed unused `config` crate dependency found during implementation

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed pre-existing formatting inconsistencies across codebase**
- **Found during:** Task 2 (cargo fmt --check verification)
- **Issue:** Pre-existing formatting issues in handlers.rs, retry.rs, server.rs, selector.rs, main.rs caused cargo fmt --check to fail
- **Fix:** Ran cargo fmt to fix all formatting
- **Files modified:** src/proxy/handlers.rs, src/proxy/retry.rs, src/proxy/server.rs, src/router/selector.rs, src/main.rs
- **Verification:** cargo fmt --check passes, all tests pass
- **Committed in:** a0323ee (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Formatting fix was necessary to pass verification. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- ApiKey type is ready for Phase 6 (env var expansion) to populate keys from environment variables
- expose_secret() pattern established for auditing all raw value access
- All 41 tests pass (33 existing + 8 new), zero clippy warnings

## Self-Check: PASSED

All files exist, all commits verified.

---
*Phase: 05-secret-type-foundation*
*Completed: 2026-02-15*
