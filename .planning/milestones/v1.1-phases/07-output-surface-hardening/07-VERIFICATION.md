---
phase: 07-output-surface-hardening
verified: 2026-02-15T23:35:00Z
status: passed
score: 7/7 must-haves verified
---

# Phase 7: Output Surface Hardening Verification Report

**Phase Goal:** All remaining output surfaces are audited and hardened -- users get actionable warnings about config hygiene and can verify key identity without seeing full keys

**Verified:** 2026-02-15T23:35:00Z

**Status:** passed

**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Starting arbstr with a config.toml that has 0644 permissions emits a warning naming the file and showing '0644' | ✓ VERIFIED | check_file_permissions() in src/config.rs (lines 225-244) called from serve command (main.rs:82-89) with format!("{:04o}", mode) |
| 2 | The /providers endpoint returns masked key prefixes like 'cashuA...***' instead of '[REDACTED]' | ✓ VERIFIED | handlers.rs:776 calls key.masked_prefix() returning "cashuA...***" format |
| 3 | The providers CLI command shows masked key prefixes for each provider that has a key | ✓ VERIFIED | main.rs:203 prints api_key.masked_prefix() for each provider |
| 4 | Starting arbstr with a provider whose api_key is a plaintext literal emits a warning recommending environment variables | ✓ VERIFIED | main.rs:107-115 matches KeySource::Literal and warns with convention_env_var_name() |
| 5 | The check command also emits a warning for plaintext literal keys | ✓ VERIFIED | main.rs:153-156 matches KeySource::Literal and prints warning with env var suggestion |
| 6 | Keys shorter than 10 characters return '[REDACTED]' instead of a masked prefix | ✓ VERIFIED | config.rs:74-76 returns "[REDACTED]" when secret.len() < 10 |
| 7 | Mock mode does not emit plaintext key warnings (key_sources is empty) | ✓ VERIFIED | main.rs:76 returns (mock_config(), vec![]) with empty key_sources, so loop at 105-125 has no iterations |

**Score:** 7/7 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| src/config.rs | check_file_permissions() function and ApiKey::masked_prefix() method | ✓ VERIFIED | check_file_permissions() at lines 225-244 (unix/non-unix variants), masked_prefix() at lines 67-78 |
| src/config.rs | ApiKey::masked_prefix() returning prefix + '...***' | ✓ VERIFIED | Line 77: format!("{}...***", &secret[..6]) for keys >= 10 chars |
| src/main.rs | RED-01 file permission warning, RED-04 literal key warning, masked prefix in providers CLI | ✓ VERIFIED | Permission warning 82-89, literal key warning 107-115, masked prefix 203 |
| src/proxy/handlers.rs | masked_prefix() call in list_providers handler | ✓ VERIFIED | Line 776: key.masked_prefix() in match expression |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| src/main.rs | src/config.rs | check_file_permissions() call in serve and check commands | ✓ WIRED | main.rs:82 and 143 both call arbstr::config::check_file_permissions() |
| src/proxy/handlers.rs | src/config.rs | ApiKey::masked_prefix() call in list_providers | ✓ WIRED | handlers.rs:776 calls key.masked_prefix() on ApiKey reference |
| src/main.rs | src/config.rs | convention_env_var_name() in RED-04 warning message | ✓ WIRED | main.rs:113-114 and 154-156 call arbstr::config::convention_env_var_name(provider_name) |

### Requirements Coverage

| Requirement | Status | Supporting Truth | Evidence |
|-------------|--------|------------------|----------|
| RED-01 | ✓ SATISFIED | Truth 1 | check_file_permissions() detects mode & 0o177 != 0, warns with octal format |
| RED-03 | ✓ SATISFIED | Truths 2, 3, 6 | masked_prefix() shows "cashuA...***" in both /providers endpoint and CLI, "[REDACTED]" for short keys |
| RED-04 | ✓ SATISFIED | Truths 4, 5, 7 | KeySource::Literal triggers warning in both serve and check commands, mock mode skips (empty key_sources) |

### Anti-Patterns Found

No anti-patterns detected. Files scanned: src/config.rs, src/main.rs, src/proxy/handlers.rs.

- No TODO/FIXME/PLACEHOLDER comments
- No empty return statements or stub implementations
- All functions have substantive implementations
- All test coverage present (7 new unit tests for masked_prefix and permission checks)

### Human Verification Required

None required. All truths are programmatically verifiable through:
- Code inspection (function exists, correct logic)
- Unit tests (64 lib tests pass, including 7 new tests)
- Integration tests (5 env var expansion tests pass)
- Static analysis (all key links confirmed via grep)

### Test Results

```
cargo test --lib
test result: ok. 64 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

cargo test --test '*'
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**Total:** 69 tests, all passing

### Commit Verification

Task commits exist and match SUMMARY.md documentation:

1. **b818ba3** - feat(07-01): add check_file_permissions and ApiKey::masked_prefix to config.rs
   - Modified: src/config.rs (+103 lines)
   - Added check_file_permissions() with unix/non-unix variants
   - Added masked_prefix() method to ApiKey
   - Added 7 unit tests

2. **804f21b** - feat(07-01): wire permission warnings, literal key warnings, and masked prefixes into CLI and API
   - Modified: src/main.rs (+37, -3), src/proxy/handlers.rs (+3, -4)
   - Wired RED-01 warnings into serve and check commands
   - Wired RED-04 warnings into serve and check commands
   - Added masked_prefix() display in providers CLI and /providers endpoint

### Code Quality

- **Clippy:** Clean (no warnings)
- **Build:** Success (cargo build --release)
- **Dependencies:** None added (uses existing std::os::unix::fs::PermissionsExt)
- **Platform gating:** Proper #[cfg(unix)] / #[cfg(not(unix))] pattern

---

**Phase 7 goal ACHIEVED.** All remaining output surfaces are hardened:

1. ✓ Users get actionable warnings about config file permissions (RED-01)
2. ✓ Users get actionable warnings about plaintext literal keys (RED-04)
3. ✓ Users can verify key identity without seeing full keys (RED-03)

All must-haves verified. No gaps found. Ready to mark phase complete.

---

_Verified: 2026-02-15T23:35:00Z_
_Verifier: Claude (gsd-verifier)_
