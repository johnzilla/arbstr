---
phase: 06-environment-variable-expansion
plan: 01
subsystem: config
tags: [env-var, expansion, convention-lookup, key-source, two-phase-config]

# Dependency graph
requires:
  - phase: 05-secret-type-foundation
    provides: "ApiKey newtype wrapping SecretString with redacted Debug/Display/Serialize"
provides:
  - "RawProviderConfig and RawConfig for two-phase config loading"
  - "expand_env_vars_with closure-based ${VAR} expansion engine"
  - "convention_env_var_name and convention_key_lookup for ARBSTR_<NAME>_API_KEY auto-discovery"
  - "KeySource enum (Literal, EnvExpanded, Convention, None) for tracking key provenance"
  - "Config::from_raw conversion from raw to final config with expansion"
  - "Config::from_file_with_env as env-var-aware file loading entry point"
  - "EnvVar error variant on ConfigError with var/provider context"
affects: [06-02-main-integration, 07-config-file-permissions]

# Tech tracking
tech-stack:
  added: []
  patterns: [two-phase-config-loading, closure-based-testable-expansion, convention-env-var-naming]

key-files:
  created: []
  modified:
    - src/config.rs

key-decisions:
  - "Made RawProviderConfig and RawConfig pub to satisfy Rust private_interfaces lint (from_raw is pub, takes RawConfig)"
  - "Used collapsible replace(['-', ' '], \"_\") per clippy suggestion for convention name transform"
  - "expand_env_vars_with uses closure-based lookup for testability; expand_env_vars wraps with std::env::var"
  - "from_file_with_env is separate entry point; existing parse_str and from_file unchanged for backward compatibility"

patterns-established:
  - "Two-phase config loading: TOML -> RawConfig -> expand -> Config with ApiKey"
  - "Closure-based env expansion: expand_env_vars_with(input, provider, lookup) for test isolation"
  - "Convention naming: ARBSTR_<UPPER_SNAKE_NAME>_API_KEY for auto-discovery"

# Metrics
duration: 3min
completed: 2026-02-15
---

# Phase 6 Plan 1: Env Var Expansion Engine Summary

**Closure-based ${VAR} expansion engine with ARBSTR_<NAME>_API_KEY convention auto-discovery, KeySource provenance tracking, and two-phase RawConfig-to-Config loading pipeline**

## Performance

- **Duration:** 3 min
- **Started:** 2026-02-15T22:47:11Z
- **Completed:** 2026-02-15T22:50:45Z
- **Tasks:** 2
- **Files modified:** 1

## Accomplishments
- Implemented `expand_env_vars_with` closure-based engine supporting single/multiple `${VAR}` expansion with clear error messages for missing vars, unclosed braces, and empty names
- Added `RawProviderConfig`/`RawConfig` structs for two-phase config loading (parse TOML as raw strings, then expand and convert)
- Added `convention_env_var_name` (public) and `convention_key_lookup` for `ARBSTR_<NAME>_API_KEY` auto-discovery
- Added `KeySource` enum with Display for tracking provenance (Literal, EnvExpanded, Convention, None)
- Added `Config::from_raw` and `Config::from_file_with_env` as new env-var-aware entry points
- Added 16 comprehensive unit tests covering all expansion, convention, and integration paths
- All 57 tests pass (41 existing + 16 new), zero clippy warnings

## Task Commits

Each task was committed atomically:

1. **Task 1: Add RawProviderConfig, env expansion engine, convention lookup, and KeySource** - `3b1eb57` (feat)
2. **Task 2: Add comprehensive unit tests for env var expansion engine** - `9a363ab` (test)

## Files Created/Modified
- `src/config.rs` - Added KeySource enum, EnvVar error variant, RawProviderConfig, RawConfig, expand_env_vars_with, expand_env_vars, convention_env_var_name, convention_key_lookup, Config::from_raw, Config::from_file_with_env, and 16 unit tests

## Decisions Made
- Made RawProviderConfig and RawConfig `pub` (plan said private) because `Config::from_raw` is `pub` and Rust's `private_interfaces` lint would fail clippy with `-D warnings`. External code uses `from_file_with_env` as the primary entry point anyway.
- Used `replace(['-', ' '], "_")` (collapsible form) per clippy suggestion instead of chained `.replace('-', "_").replace(' ', "_")`

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Made RawProviderConfig and RawConfig pub for clippy compliance**
- **Found during:** Task 1 (compilation verification)
- **Issue:** Plan specified RawProviderConfig and RawConfig as private, but `Config::from_raw` is public and takes `RawConfig` as argument. Rust's `private_interfaces` lint produces a warning, which fails `clippy -- -D warnings`.
- **Fix:** Changed both structs from private to `pub`. Fields remain private; external code uses `from_file_with_env` instead of constructing RawConfig directly.
- **Files modified:** src/config.rs
- **Verification:** clippy passes clean with `-D warnings`
- **Committed in:** 3b1eb57 (Task 1 commit)

**2. [Rule 1 - Bug] Used collapsible str::replace per clippy**
- **Found during:** Task 1 (clippy verification)
- **Issue:** Consecutive `.replace('-', "_").replace(' ', "_")` triggers `clippy::collapsible_str_replace`
- **Fix:** Changed to `.replace(['-', ' '], "_")`
- **Files modified:** src/config.rs
- **Verification:** clippy passes clean
- **Committed in:** 3b1eb57 (Task 1 commit)

---

**Total deviations:** 2 auto-fixed (2 bugs/lint fixes)
**Impact on plan:** Both fixes necessary for clippy compliance. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Expansion engine is complete and ready for Plan 02 (main.rs integration)
- `Config::from_file_with_env` returns `(Config, Vec<(String, KeySource)>)` for startup logging
- `convention_env_var_name` is public for check command reporting
- All existing tests pass unchanged (backward compatibility confirmed)
- 57 total tests provide strong regression safety net

## Self-Check: PASSED

All files exist, all commits verified.

---
*Phase: 06-environment-variable-expansion*
*Completed: 2026-02-15*
