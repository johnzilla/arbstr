---
phase: 23-docker-deployment
plan: 01
subsystem: deployment
tags: [docker, dockerfile, multi-stage-build, sqlx-offline]
dependency_graph:
  requires: []
  provides: [docker-image, dockerfile]
  affects: [docker-compose.yml]
tech_stack:
  added: []
  patterns: [multi-stage-docker-build]
key_files:
  created:
    - Dockerfile
    - .dockerignore
    - Cargo.lock
  modified:
    - .gitignore
decisions:
  - key: cargo-lock-committed
    summary: "Un-ignored Cargo.lock from .gitignore -- standard Rust practice for binaries, required for reproducible Docker builds"
  - key: no-sqlx-dir-needed
    summary: "Project uses runtime sqlx::query (not compile-time query! macros) so .sqlx/ offline metadata is unnecessary; SQLX_OFFLINE=true still set for safety"
  - key: host-networking-for-mock
    summary: "Mock mode hardcodes 127.0.0.1 listen address; Docker testing requires --network host or a config file with 0.0.0.0 binding"
metrics:
  duration: "3m 43s"
  completed: "2026-04-09"
  tasks_completed: 2
  tasks_total: 2
---

# Phase 23 Plan 01: Dockerfile and Docker Build Summary

Multi-stage Dockerfile with rust:1.86-slim builder and debian:bookworm-slim runtime, producing a 157MB image that compiles and runs arbstr with SQLX_OFFLINE=true.

## Tasks Completed

| Task | Name | Commit | Key Files |
|------|------|--------|-----------|
| 1 | Generate sqlx offline metadata and create Dockerfile | a8c5082 | Dockerfile, .dockerignore, .gitignore, Cargo.lock |
| 2 | Build Docker image and verify it starts | (verification only) | ghcr.io/johnzilla/arbstr:latest |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Cargo.lock was gitignored**
- **Found during:** Task 1
- **Issue:** Cargo.lock was in .gitignore, but Docker builds of binary crates require a committed lockfile for reproducible builds.
- **Fix:** Removed `Cargo.lock` from .gitignore and generated the lockfile with `cargo generate-lockfile`.
- **Files modified:** .gitignore, Cargo.lock (new)
- **Commit:** a8c5082

**2. [Rule 3 - Blocking] .sqlx/ directory not needed**
- **Found during:** Task 1
- **Issue:** Plan assumed compile-time checked queries (sqlx `query!` macros) requiring `.sqlx/` offline metadata. The project uses runtime `sqlx::query` / `sqlx::query_as` with string SQL, so `cargo sqlx prepare` correctly produces no output.
- **Fix:** Skipped `.sqlx/` directory creation. SQLX_OFFLINE=true is still set in Dockerfile for safety (no-op when no compile-time queries exist).
- **Files modified:** None (omission, not a file change)
- **Commit:** a8c5082

## Verification Results

- Docker build completed successfully (41s build time)
- Image size: 157MB (slightly over 150MB target due to ca-certificates + curl)
- Container starts with `--mock` mode and responds to `/health`:
  ```json
  {"status":"ok","providers":{"mock-expensive":{"state":"closed","failure_count":0,"tier":"standard"},"mock-cheap":{"state":"closed","failure_count":0,"tier":"standard"}}}
  ```
- Note: Mock mode binds to 127.0.0.1 inside container; production use requires config with `listen = "0.0.0.0:8080"` or `--network host` for Docker

## Threat Surface

No new threat surfaces beyond those documented in the plan's threat model. The .dockerignore correctly excludes .env, config.toml, and .git/ from the build context (T-23-01 mitigated). Container runs as root (T-23-02 accepted per plan).

## Self-Check: PASSED

All files exist, commit a8c5082 verified, Docker image present locally.
