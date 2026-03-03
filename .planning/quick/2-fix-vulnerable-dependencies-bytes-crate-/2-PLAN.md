---
phase: quick
plan: 2
type: execute
wave: 1
depends_on: []
files_modified:
  - Cargo.toml
  - Cargo.lock
autonomous: true
requirements: [SEC-01, SEC-02]

must_haves:
  truths:
    - "bytes crate is at 1.11.1+ (patched for RUSTSEC-2026-0007)"
    - "rsa crate no longer appears in Cargo.lock"
    - "All existing tests pass with updated dependencies"
  artifacts:
    - path: "Cargo.toml"
      provides: "Pinned bytes version >= 1.11.1"
      contains: 'bytes = "1.11'
    - path: "Cargo.lock"
      provides: "Clean lockfile without stale sqlx-mysql/sqlx-postgres/rsa entries"
  key_links:
    - from: "Cargo.toml"
      to: "Cargo.lock"
      via: "cargo update regeneration"
      pattern: 'bytes.*1\.11\.[1-9]'
---

<objective>
Fix two vulnerable dependencies flagged by cargo audit:

1. **RUSTSEC-2026-0007 (Critical):** Integer overflow in `BytesMut::reserve` in bytes 1.11.0. Update to >= 1.11.1.
2. **RUSTSEC-2023-0071 (Medium):** Marvin Attack in rsa 0.9.10, pulled in via stale `sqlx-mysql` entry in Cargo.lock. The project only uses SQLite -- sqlx-mysql is an optional dependency of sqlx-macros-core that is never activated. Regenerating the lockfile removes it.

Purpose: Eliminate known security vulnerabilities from the dependency tree.
Output: Updated Cargo.toml and Cargo.lock with no audit warnings.
</objective>

<execution_context>
@/home/john/.claude/get-shit-done/workflows/execute-plan.md
@/home/john/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@Cargo.toml
@Cargo.lock (bytes and rsa entries)
</context>

<tasks>

<task type="auto">
  <name>Task 1: Update bytes crate and regenerate lockfile</name>
  <files>Cargo.toml, Cargo.lock</files>
  <action>
1. In Cargo.toml, change `bytes = "1"` to `bytes = "1.11.1"` to pin the minimum version to the patched release. This prevents any resolver from selecting 1.11.0 or earlier.

2. Delete Cargo.lock entirely, then run `cargo generate-lockfile` to produce a clean lockfile. This removes stale entries for sqlx-mysql, sqlx-postgres, and rsa that were carried forward from previous resolves when sqlx may have had different default features. The project only declares `features = ["runtime-tokio", "sqlite", "migrate"]` for sqlx, so the mysql/postgres optional deps should not appear in a fresh resolve.

3. Verify the fixes:
   - Run `grep 'name = "rsa"' Cargo.lock` -- should return nothing
   - Run `grep 'name = "sqlx-mysql"' Cargo.lock` -- should return nothing
   - Run `grep -A2 'name = "bytes"' Cargo.lock` -- should show version >= 1.11.1
   - If cargo-audit is installed, run `cargo audit` to confirm zero vulnerabilities

4. Run `cargo build` to confirm the project compiles with the updated dependencies.

5. Run `cargo test` to confirm all existing tests pass.

6. Run `cargo clippy -- -D warnings` to confirm no new warnings introduced.
  </action>
  <verify>
    <automated>cargo test 2>&1 && cargo clippy -- -D warnings 2>&1 && grep -A2 'name = "bytes"' Cargo.lock | grep -q '1.11.[1-9]' && echo "BYTES OK" && ! grep -q 'name = "rsa"' Cargo.lock && echo "RSA REMOVED" && echo "ALL CHECKS PASSED"</automated>
  </verify>
  <done>
    - bytes crate resolved to >= 1.11.1 in Cargo.lock (RUSTSEC-2026-0007 fixed)
    - rsa crate absent from Cargo.lock (RUSTSEC-2023-0071 eliminated)
    - sqlx-mysql and sqlx-postgres absent from Cargo.lock
    - All tests pass
    - No clippy warnings
  </done>
</task>

</tasks>

<verification>
- `cargo audit` (if installed) reports zero vulnerabilities
- `cargo test` passes all existing tests
- `cargo clippy -- -D warnings` passes
- `grep 'name = "rsa"' Cargo.lock` returns no results
- `grep -A2 'name = "bytes"' Cargo.lock` shows version >= 1.11.1
</verification>

<success_criteria>
Both RUSTSEC-2026-0007 (bytes) and RUSTSEC-2023-0071 (rsa) are resolved. All tests pass. No new warnings.
</success_criteria>

<output>
After completion, create `.planning/quick/2-fix-vulnerable-dependencies-bytes-crate-/2-SUMMARY.md`
</output>
