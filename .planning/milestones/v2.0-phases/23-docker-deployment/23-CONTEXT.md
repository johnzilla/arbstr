# Phase 23: Docker Deployment - Context

**Gathered:** 2026-04-09
**Status:** Ready for planning

<domain>
## Phase Boundary

Create a multi-stage Dockerfile for arbstr core and harden the arbstr-node docker-compose.yml so the full stack (core + vault + LND + Cashu mint) starts cleanly from `docker compose up`.

</domain>

<decisions>
## Implementation Decisions

### Dockerfile Location
- **D-01:** Dockerfile lives in the arbstr (core) repo for CI and local dev builds. arbstr-node's docker-compose.yml references the pre-built image from the registry.
- **D-02:** Both repos have their own Dockerfile/image reference: arbstr builds and pushes, arbstr-node pulls.

### Build Strategy
- **D-03:** Multi-stage Dockerfile: rust:1.86-slim as builder, debian:bookworm-slim as runtime. Produces a minimal image with just the binary.
- **D-04:** `SQLX_OFFLINE=true` required for Docker builds. Must run `cargo sqlx prepare` before building to generate query metadata. Include .sqlx/ directory in build context.
- **D-05:** amd64 only for now. No multi-arch buildx complexity.

### Image Registry
- **D-06:** Publish to GitHub Container Registry (ghcr.io/johnzilla/arbstr). Free for public repos, integrates with GitHub Actions.
- **D-07:** arbstr-node docker-compose.yml updated to reference `ghcr.io/johnzilla/arbstr:latest` instead of `build: context: ./core`.

### Compose Hardening
- **D-08:** Verify LND and Nutshell image versions exist on Docker Hub before pinning (research flagged v0.20.1-beta and v0.20.0 need verification).
- **D-09:** Health check chain must be tested: lnd → mint → vault → core startup order with dependency conditions.
- **D-10:** Test from clean volumes — `docker compose down -v && docker compose up` must work without manual steps.

### Claude's Discretion
- Exact Rust version in Dockerfile (1.86 or latest stable)
- Whether to add a .dockerignore file
- GitHub Actions workflow for automated image builds (if time permits)
- Whether vault image needs a Dockerfile in arbstr-vault repo (check if one exists)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Docker Configuration
- `docker-compose.yml` (this repo) — existing scaffold with 4 services
- `/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml` — arbstr-node compose file (just created)
- `/home/john/vault/projects/github.com/arbstr-node/config.toml` — core config with vault wired
- `/home/john/vault/projects/github.com/arbstr-node/.env.example` — environment variables

### Build Dependencies
- `Cargo.toml` — dependencies and build configuration
- `migrations/` — SQLite migrations (embedded, need .sqlx/ for offline mode)

### Research
- `.planning/research/STACK.md` — Docker image version recommendations, Dockerfile patterns
- `.planning/research/PITFALLS.md` — Pitfall 14 (mesh-llm localhost unreachable from Docker)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `docker-compose.yml` (this repo) — full 4-service scaffold with health checks, volumes, dependency ordering
- `arbstr-node/docker-compose.yml` — clean compose file referencing core + vault + lnd + mint
- `.env.example` — environment variable templates in both repos

### Established Patterns
- Health checks use `curl -f http://localhost:{port}/health` for core and vault
- LND uses `lncli --network=regtest getinfo` for health check
- Mint uses `curl -f http://localhost:3338/v1/info`

### Integration Points
- Dockerfile goes in repo root (standard location)
- .dockerignore should exclude .planning/, target/, .git/
- arbstr-node compose needs image reference update from `build:` to `image:`

</code_context>

<specifics>
## Specific Ideas

- GHCR for image hosting (ghcr.io/johnzilla/arbstr)
- amd64 only, multi-arch later if needed
- sqlx offline mode for Docker builds

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 23-docker-deployment*
*Context gathered: 2026-04-09*
