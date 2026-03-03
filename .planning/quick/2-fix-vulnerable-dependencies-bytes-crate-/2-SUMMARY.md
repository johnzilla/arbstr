---
phase: quick
plan: 2
subsystem: infra
tags: [security, cargo-audit, bytes, dependencies]

# Dependency graph
requires: []
provides:
  - "Patched bytes crate (>= 1.11.1) fixing RUSTSEC-2026-0007"
affects: []

# Tech tracking
tech-stack:
  added: []
  patterns: []

key-files:
  created: []
  modified:
    - Cargo.toml

key-decisions:
  - "Pin bytes = 1.11.1 minimum in Cargo.toml rather than relying on lockfile version"
  - "RUSTSEC-2023-0071 (rsa) acknowledged as unfixable -- no patched version exists, crate is never compiled (sqlx-mysql optional dep never activated)"

patterns-established: []

requirements-completed: [SEC-01, SEC-02]

# Metrics
duration: 2min
completed: 2026-03-03
---

# Quick Task 2: Fix Vulnerable Dependencies Summary

**Pinned bytes >= 1.11.1 to fix RUSTSEC-2026-0007 integer overflow; acknowledged unfixable RUSTSEC-2023-0071 (rsa) as non-compiled transitive dep**

## Performance

- **Duration:** 2 min
- **Started:** 2026-03-03T00:43:53Z
- **Completed:** 2026-03-03T00:46:00Z
- **Tasks:** 1
- **Files modified:** 1

## Accomplishments
- Pinned `bytes = "1.11.1"` in Cargo.toml to prevent resolving the vulnerable 1.11.0 release (RUSTSEC-2026-0007: integer overflow in `BytesMut::reserve`)
- Regenerated Cargo.lock confirms bytes 1.11.1 resolved
- All 194 tests pass with updated dependencies
- Zero clippy warnings
- `cargo audit --ignore RUSTSEC-2023-0071` passes clean

## Task Commits

Each task was committed atomically:

1. **Task 1: Update bytes crate and regenerate lockfile** - `aa91f8c` (fix)

## Files Created/Modified
- `Cargo.toml` - Changed `bytes = "1"` to `bytes = "1.11.1"` to pin minimum patched version
- `Cargo.lock` - Regenerated (gitignored, not committed, but resolves bytes 1.11.1)

## Decisions Made

1. **Pin bytes minimum version in Cargo.toml** -- Rather than relying on the lockfile to hold a specific version, pinning `bytes = "1.11.1"` in Cargo.toml ensures any future resolve (including on CI or other machines) will always pick the patched version.

2. **RUSTSEC-2023-0071 (rsa) acknowledged as unfixable** -- The plan assumed regenerating the lockfile would remove the rsa crate by dropping stale sqlx-mysql entries. In practice, Cargo's resolver includes all optional dependencies of transitive crates in the lockfile even when those features are not activated. `cargo tree -i rsa` confirms "nothing to print" (rsa is never compiled). The advisory states "No fixed upgrade is available!" Since the crate is never compiled and no fix exists, this is a known accepted risk.

## Deviations from Plan

### Plan Assumptions vs Reality

**1. [Rule 3 - Blocking] Lockfile regeneration does not remove sqlx-mysql/rsa**
- **Found during:** Task 1 (verification step)
- **Issue:** The plan stated "Regenerating the lockfile removes it" for sqlx-mysql/rsa/sqlx-postgres entries. In reality, the Cargo resolver includes all optional dependencies of transitive crates in the lockfile regardless of feature activation. `sqlx-macros-core` unconditionally lists `sqlx-mysql` and `sqlx-postgres` as dependencies.
- **Fix:** Verified via `cargo tree -i rsa` and `cargo tree -i sqlx-mysql` that these crates are never compiled. `cargo audit --ignore RUSTSEC-2023-0071` passes clean. The rsa advisory has no fix available upstream.
- **Impact:** The "rsa absent from Cargo.lock" done criterion cannot be met through any project-level change. The vulnerability exists only in the lockfile metadata, not in compiled code.
- **Verification:** `cargo tree -i rsa` outputs "nothing to print" confirming rsa is never in the build graph

---

**Total deviations:** 1 (plan assumption incorrect about lockfile behavior)
**Impact on plan:** RUSTSEC-2026-0007 (bytes) fully fixed. RUSTSEC-2023-0071 (rsa) cannot be fixed at project level -- no patched rsa version exists, and the crate is never compiled. This is an upstream sqlx-macros-core packaging issue.

## Issues Encountered
- Cargo.lock is gitignored in this project, so the regenerated lockfile is not tracked in version control. The Cargo.toml pin ensures the correct version is resolved regardless.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Dependencies are clean for the bytes vulnerability
- rsa advisory will resolve when sqlx upstream either drops the mysql optional dep from macros-core or rsa publishes a fix

## Self-Check: PASSED

- FOUND: 2-SUMMARY.md
- FOUND: aa91f8c (task commit)
- FOUND: bytes = "1.11.1" pin in Cargo.toml
- FOUND: bytes 1.11.1 resolved in Cargo.lock

---
*Phase: quick*
*Completed: 2026-03-03*
