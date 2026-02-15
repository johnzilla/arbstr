---
phase: 07-output-surface-hardening
plan: 01
subsystem: config
tags: [security, secrets, permissions, redaction, cli]

# Dependency graph
requires:
  - phase: 05-secret-type-foundation
    provides: "ApiKey wrapper with SecretString, expose_secret() pattern"
  - phase: 06-environment-variable-expansion
    provides: "KeySource enum, convention_env_var_name(), from_file_with_env()"
provides:
  - "check_file_permissions() function for config file permission auditing"
  - "ApiKey::masked_prefix() for safe key identity display"
  - "RED-01 file permission warnings in serve and check commands"
  - "RED-03 masked key prefixes in /providers endpoint and providers CLI"
  - "RED-04 plaintext literal key warnings with env var suggestions"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns:
    - "cfg(unix)/cfg(not(unix)) gating for platform-specific permission checks"
    - "masked_prefix() as separate method from Debug/Display/Serialize (which remain [REDACTED])"

key-files:
  created: []
  modified:
    - "src/config.rs"
    - "src/main.rs"
    - "src/proxy/handlers.rs"

key-decisions:
  - "6-char prefix chosen for masked_prefix to identify cashuA tokens without revealing content"
  - "Keys < 10 chars fall back to [REDACTED] to avoid exposing most of a short key"
  - "Permission check returns Option so caller controls warning format (tracing vs println)"
  - "Mock mode skips permission check and literal key warnings (no config file, empty key_sources)"

patterns-established:
  - "Platform-gated functions: #[cfg(unix)] with #[cfg(not(unix))] no-op fallback"
  - "Masked prefix pattern: expose enough for identification, redact remainder"

# Metrics
duration: 3min
completed: 2026-02-15
---

# Phase 7 Plan 1: Output Surface Hardening Summary

**File permission warnings, masked key prefixes (cashuA...***), and plaintext literal key warnings across all CLI and API output surfaces**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-15T23:24:58Z
- **Completed:** 2026-02-15T23:27:58Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- check_file_permissions() detects overly permissive config files (> 0600) on Unix with no-op fallback on other platforms
- ApiKey::masked_prefix() returns distinguishable prefixes like "cashuA...***" for normal keys, "[REDACTED]" for short keys
- Serve and check commands emit actionable warnings for both file permissions and plaintext literal API keys
- /providers endpoint and providers CLI show masked key prefixes instead of uniform "[REDACTED]"
- Mock mode produces no spurious warnings (empty key_sources, no config file to check)

## Task Commits

Each task was committed atomically:

1. **Task 1: Add check_file_permissions and masked_prefix to config.rs** - `b818ba3` (feat)
2. **Task 2: Wire warnings and masked prefix into CLI commands and /providers endpoint** - `804f21b` (feat)

## Files Created/Modified
- `src/config.rs` - Added check_file_permissions() function (unix/non-unix), ApiKey::masked_prefix() method, 7 unit tests
- `src/main.rs` - Added RED-01 permission warnings, RED-04 literal key warnings in serve/check commands, masked key in providers command
- `src/proxy/handlers.rs` - Changed /providers endpoint from "[REDACTED]" to masked_prefix() output

## Decisions Made
- 6-character prefix chosen for masked_prefix() -- enough to identify cashuA tokens without revealing content
- Keys shorter than 10 characters return "[REDACTED]" to avoid exposing most of a short key
- Permission check returns Option<(String, u32)> so caller controls warning format (tracing::warn vs println)
- Mock mode correctly skips all permission and literal key warnings since there is no config file and key_sources is empty

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness
- All RED-01 (file permissions), RED-03 (masked key prefix), and RED-04 (literal key warning) requirements are satisfied
- v1.1 Secrets Hardening milestone is now feature-complete across all output surfaces
- 64 lib tests + 5 integration tests pass, clippy clean, release build compiles

## Self-Check: PASSED

All files exist, all commits verified, all key functions present in expected locations.

---
*Phase: 07-output-surface-hardening*
*Completed: 2026-02-15*
