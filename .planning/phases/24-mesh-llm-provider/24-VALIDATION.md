---
phase: 24
slug: mesh-llm-provider
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-10
---

# Phase 24 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml |
| **Quick run command** | `cargo test --lib` |
| **Full suite command** | `cargo test` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --lib`
- **After every plan wave:** Run `cargo test`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 24-01-01 | 01 | 1 | MESH-01 | — | N/A | unit | `cargo test config` | ❌ W0 | ⬜ pending |
| 24-01-02 | 01 | 1 | MESH-02 | — | N/A | unit | `cargo test discover` | ❌ W0 | ⬜ pending |
| 24-01-03 | 01 | 1 | MESH-03 | — | N/A | integration | `cargo test mesh` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `tests/discovery.rs` — integration tests for auto-discovery
- [ ] Unit tests in `src/config.rs` for `auto_discover` field deserialization

*Existing test infrastructure (cargo test, mock providers) covers framework requirements.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Docker Compose mesh-llm host access | MESH-03 | Requires Docker environment with host networking | Run `docker compose up`, verify core can reach host.docker.internal:9337 |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
