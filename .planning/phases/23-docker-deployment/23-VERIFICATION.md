---
phase: 23-docker-deployment
verified: 2026-04-09T20:00:00Z
status: gaps_found
score: 1/3 must-haves verified
overrides_applied: 0
gaps:
  - truth: "docker compose up from empty volumes starts lnd, mint, vault, and core in correct dependency order with health checks"
    status: partial
    reason: "Compose config and health check chain are structurally correct (lnd+mint -> vault -> core), but vault service fails to stay up due to FST_ERR_INSTANCE_ALREADY_LISTENING bug in arbstr-vault code (separate repo). 3 of 4 services start cleanly; vault blocks core from starting."
    artifacts:
      - path: "/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml"
        issue: "File is correct — health chain, image references, and env vars are properly configured. The gap is a runtime bug in the vault service image, not a compose config issue."
    missing:
      - "Fix FST_ERR_INSTANCE_ALREADY_LISTENING bug in arbstr-vault startup code (separate repo: arbstr-node/vault)"
      - "Re-run human verification checkpoint (Task 2 of Plan 02) after vault fix to confirm all 4 services reach healthy state"

  - truth: "A chat completion request through the composed stack returns a successful response with billing headers"
    status: failed
    reason: "Full-stack end-to-end test was never performed. The human verification checkpoint (Task 2, Plan 02) only tested service startup order, not a live inference request with billing. Vault code bug further blocks this test."
    artifacts:
      - path: "/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml"
        issue: "Stack does not reach fully healthy state due to vault bug, making this test impossible without fixing the vault service first."
    missing:
      - "Fix vault startup bug (FST_ERR_INSTANCE_ALREADY_LISTENING)"
      - "Run a test POST /v1/chat/completions request through the composed stack and verify 200 response with X-Arbstr-Cost and X-Arbstr-Provider headers"
---

# Phase 23: Docker Deployment Verification Report

**Phase Goal:** arbstr-node runs as a complete stack from a single docker compose up command
**Verified:** 2026-04-09T20:00:00Z
**Status:** gaps_found
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (from ROADMAP.md Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Multi-stage Dockerfile produces a slim arbstr core image (builder stage + runtime stage) | VERIFIED | Dockerfile exists at repo root. `FROM rust:1.86-slim AS builder` (line 2) + `FROM debian:bookworm-slim` (line 12). `SQLX_OFFLINE=true` set. Committed in a8c5082. Image built successfully (157MB). |
| 2 | docker compose up from empty volumes starts lnd, mint, vault, and core in correct dependency order with health checks | PARTIAL | Compose config is structurally correct: health chain lnd+mint -> vault -> core via `condition: service_healthy` confirmed in docker-compose.yml. 3 of 4 services start; vault fails at runtime due to FST_ERR_INSTANCE_ALREADY_LISTENING bug in arbstr-vault code (separate repo). Core never reaches healthy because vault never becomes healthy. |
| 3 | A chat completion request through the composed stack returns a successful response with billing headers | FAILED | No evidence of end-to-end test. Human verification checkpoint (Plan 02 Task 2) tested startup order only, not a live request. Vault bug prevents full-stack test. |

**Score:** 1/3 truths fully verified

### Deferred Items

None. No later phases cover the vault startup bug fix or end-to-end compose test.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `Dockerfile` | Multi-stage build for arbstr core | VERIFIED | Exists at repo root. Builder: `rust:1.86-slim`. Runtime: `debian:bookworm-slim`. `SQLX_OFFLINE=true`. Committed in a8c5082. |
| `.dockerignore` | Build context exclusions | VERIFIED | Exists. Contains `target/`, `.git/`, `.env`, `config.toml` and other exclusions. |
| `Cargo.lock` | Committed lockfile for reproducible builds | VERIFIED | Un-ignored and committed in a8c5082 (was previously gitignored). |
| `/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml` | Full-stack compose with GHCR image and health chain | VERIFIED (config) | `ghcr.io/johnzilla/arbstr:latest` for core service. Health chain enforced via `condition: service_healthy`. LND v0.20.1-beta, Nutshell 0.20.0. `extra_hosts` and `VAULT_INTERNAL_TOKEN` present. Committed in bebcd5d. |

Note: `.sqlx/` directory was correctly determined to be unnecessary — the project uses runtime `sqlx::query` (not compile-time `query!` macros), so offline metadata is not required. SQLX_OFFLINE=true is still set in Dockerfile as a no-op safety measure.

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| core service | vault service | `depends_on: vault: condition: service_healthy` | WIRED | Line 103-105 of docker-compose.yml |
| vault service | lnd service | `depends_on: lnd: condition: service_healthy` | WIRED | Lines 78-80 of docker-compose.yml |
| vault service | mint service | `depends_on: mint: condition: service_healthy` | WIRED | Lines 81-83 of docker-compose.yml |
| core service | GHCR image | `image: ghcr.io/johnzilla/arbstr:latest` | WIRED | Line 91 of docker-compose.yml |
| Dockerfile builder | SQLX_OFFLINE | `ENV SQLX_OFFLINE=true` | WIRED | Line 8 of Dockerfile |

### Data-Flow Trace (Level 4)

Not applicable — this phase produces infrastructure artifacts (Dockerfile, docker-compose.yml), not dynamic data-rendering components.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| docker image builds from Dockerfile | `docker build` | Succeeded per SUMMARY (157MB image) | PASS (documented) |
| GHCR image reference in compose | `grep "ghcr.io/johnzilla/arbstr" docker-compose.yml` | Match found | PASS |
| Health chain conditions present | `grep "condition: service_healthy" docker-compose.yml` | 3 matches (lnd, mint, vault) | PASS |
| Full stack starts (4 services healthy) | `docker compose up -d && docker compose ps` | FAIL — vault bug FST_ERR_INSTANCE_ALREADY_LISTENING prevents vault from staying up | FAIL |
| End-to-end chat completion with billing headers | `curl POST /v1/chat/completions` through compose stack | NOT TESTED — blocked by vault bug | FAIL |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| DEPLOY-01 | 23-01-PLAN.md | Multi-stage Dockerfile for arbstr core (Rust builder + slim runtime) | SATISFIED | Dockerfile exists, multi-stage, builds successfully (a8c5082) |
| DEPLOY-02 | 23-02-PLAN.md | Docker Compose health check chain verified (lnd -> mint -> vault -> core startup order) | PARTIAL | Compose config is correct; runtime blocked by vault code bug (separate repo) |
| DEPLOY-03 | 23-02-PLAN.md | Full stack starts cleanly from empty volumes with `docker compose up` | BLOCKED | Vault service fails to start; human verification checkpoint was not completed |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| docker-compose.yml | — | vault uses `build: context: ./vault` | Info | Requires arbstr-vault repo cloned as `./vault` subdirectory of arbstr-node. Not a stub, but requires setup documentation. |

No TODO/FIXME/placeholder patterns found in Dockerfile or docker-compose.yml.

### Human Verification Required

Plan 02 Task 2 is an explicitly gated human verification checkpoint that was never completed due to the vault startup bug:

**1. Full-stack startup verification**

**Test:** After fixing vault startup bug, run:
```
cd /home/john/vault/projects/github.com/arbstr-node
cp .env.example .env  # fill in secrets
docker compose down -v
docker compose up -d
docker compose ps  # all 4 services must show "healthy"
```
**Expected:** All 4 services (lnd, mint, vault, core) show `healthy` status. Startup order follows lnd+mint -> vault -> core.
**Why human:** Requires running Docker Compose on local machine; can't verify via static code analysis.

**2. End-to-end chat completion with billing headers**

**Test:** With full stack running and a wallet configured in vault, send:
```
curl -X POST http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <agent-token>" \
  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"Hello"}]}'
```
**Expected:** 200 response with billing headers (X-Arbstr-Cost, X-Arbstr-Provider). Vault reserve/settle cycle completes.
**Why human:** Requires live stack with real or simulated payments; end-to-end behavior can't be statically verified.

### Gaps Summary

Two gaps block goal achievement:

**Gap 1 (Partial): Vault startup bug prevents full stack**

The docker-compose.yml infrastructure is correctly configured — health chain ordering, image references, environment variables, and dependency conditions are all present and wired. However, the vault service fails at runtime with `FST_ERR_INSTANCE_ALREADY_LISTENING` in the arbstr-vault Node.js code (separate repo). This is a bug in vault's startup logic, not a compose configuration issue. 3 of 4 services start cleanly; vault's failure cascades to block core from starting.

Fix is in the arbstr-vault repo (not arbstr or arbstr-node). Once fixed, the human verification checkpoint (Plan 02 Task 2) must be re-run to confirm all 4 services reach healthy state.

**Gap 2 (Failed): End-to-end request through composed stack not tested**

The phase's third roadmap success criterion — a live chat completion request returning a successful response with billing headers — was never tested. The human verification checkpoint in Plan 02 only checked service startup order. The end-to-end billing flow through a composed stack remains unverified. This requires the vault bug to be fixed first, then an explicit end-to-end test.

---

_Verified: 2026-04-09T20:00:00Z_
_Verifier: Claude (gsd-verifier)_
