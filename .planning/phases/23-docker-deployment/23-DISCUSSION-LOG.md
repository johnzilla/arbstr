# Phase 23: Docker Deployment - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-04-09
**Phase:** 23-docker-deployment
**Areas discussed:** Dockerfile location, Build strategy, Image registry

---

## Dockerfile Location

| Option | Description | Selected |
|--------|-------------|----------|
| In arbstr repo | Dockerfile here, arbstr-node uses pre-built image | |
| In arbstr-node | Self-contained with git clone during build | |
| In both | arbstr has Dockerfile for CI, arbstr-node references pre-built image | ✓ |

**User's choice:** Dockerfile in both repos

---

## Build Strategy — Architecture

| Option | Description | Selected |
|--------|-------------|----------|
| amd64 only | Simplest, x86_64 servers | ✓ |
| Multi-arch | amd64 + arm64 via buildx | |
| You decide | Claude picks | |

**User's choice:** amd64 only

---

## Image Registry

| Option | Description | Selected |
|--------|-------------|----------|
| GitHub Container Registry | ghcr.io/johnzilla/arbstr, free for public repos | ✓ |
| Docker Hub | docker.io/arbstr/core, more discoverable | |
| Local build only | No registry, build from source | |

**User's choice:** GitHub Container Registry

---

## Claude's Discretion

- Exact Rust version, .dockerignore, GitHub Actions workflow, vault Dockerfile check

## Deferred Ideas

None
