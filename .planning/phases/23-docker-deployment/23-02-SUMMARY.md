---
phase: 23-docker-deployment
plan: 02
subsystem: infra
tags: [docker, docker-compose, ghcr, lnd, nutshell, cashu]

requires:
  - phase: 23-docker-deployment/01
    provides: Multi-stage Dockerfile for arbstr core image
provides:
  - Updated docker-compose.yml with GHCR image reference for core service
  - Verified LND v0.20.1-beta and Nutshell 0.20.0 image tags
  - Health check dependency chain (lnd+mint -> vault -> core)
  - extra_hosts for host.docker.internal mesh-llm access
affects: []

tech-stack:
  added: []
  patterns:
    - GHCR image reference instead of local build context for core service
    - extra_hosts for host network access from containers on Linux

key-files:
  created: []
  modified:
    - /home/john/vault/projects/github.com/arbstr-node/docker-compose.yml

key-decisions:
  - "Upgraded LND from v0.18.4-beta to v0.20.1-beta (verified on Docker Hub)"
  - "Upgraded Nutshell from 0.16.3 to 0.20.0 (verified on Docker Hub)"
  - "Fixed VAULTWARDEN_ADMIN_TOKEN env var name to VAULT_ADMIN_TOKEN to match .env.example"

patterns-established:
  - "GHCR image for core: ghcr.io/johnzilla/arbstr:latest replaces local build context"

requirements-completed: [DEPLOY-02, DEPLOY-03]

duration: 3min
completed: 2026-04-09
---

# Phase 23 Plan 02: Compose GHCR Integration Summary

**Docker-compose updated with GHCR core image, verified LND v0.20.1-beta and Nutshell 0.20.0, health chain enforcement, and extra_hosts for mesh-llm**

## Performance

- **Duration:** 3 min
- **Started:** 2026-04-09T14:27:35Z
- **Completed:** 2026-04-09T14:30:35Z
- **Tasks:** 1 of 2 (Task 2 is human verification checkpoint)
- **Files modified:** 1

## Accomplishments
- Core service switched from local build to ghcr.io/johnzilla/arbstr:latest image
- LND upgraded to v0.20.1-beta and Nutshell to 0.20.0 (both verified on Docker Hub)
- Added extra_hosts for host.docker.internal to enable mesh-llm access on Linux
- Added VAULT_INTERNAL_TOKEN to core service environment for vault auth
- Fixed VAULTWARDEN_ADMIN_TOKEN typo to VAULT_ADMIN_TOKEN

## Task Commits

Each task was committed atomically:

1. **Task 1: Verify image versions and update arbstr-node docker-compose.yml** - `bebcd5d` (feat) [arbstr-node repo]

**Note:** Task 2 is a human verification checkpoint (docker compose up from clean volumes).

## Files Created/Modified
- `/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml` - Updated core to GHCR image, upgraded LND/Nutshell versions, added extra_hosts and VAULT_INTERNAL_TOKEN

## Decisions Made
- Upgraded LND from v0.18.4-beta to v0.20.1-beta -- newer version verified on Docker Hub via docker manifest inspect
- Upgraded Nutshell from 0.16.3 to 0.20.0 -- newer version verified on Docker Hub via docker manifest inspect
- Fixed VAULTWARDEN_ADMIN_TOKEN to VAULT_ADMIN_TOKEN -- matched .env.example naming convention

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed VAULTWARDEN_ADMIN_TOKEN env var name**
- **Found during:** Task 1
- **Issue:** Vault service had `VAULTWARDEN_ADMIN_TOKEN=${VAULT_ADMIN_TOKEN}` -- the container env var name was a leftover from vaultwarden, should be VAULT_ADMIN_TOKEN to match .env.example and vault service expectations
- **Fix:** Renamed to `VAULT_ADMIN_TOKEN=${VAULT_ADMIN_TOKEN}`
- **Files modified:** docker-compose.yml
- **Verification:** Consistent with .env.example which defines VAULT_ADMIN_TOKEN
- **Committed in:** bebcd5d

---

**Total deviations:** 1 auto-fixed (1 bug fix)
**Impact on plan:** Minor naming fix for env var consistency. No scope creep.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Task 2 (human verification) pending: user needs to run docker compose up from clean volumes
- Once verified, full-stack deployment is confirmed working

---
*Phase: 23-docker-deployment*
*Completed: 2026-04-09*
