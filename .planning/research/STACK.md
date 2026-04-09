# Technology Stack

**Project:** arbstr v2.0 Inference Marketplace Foundation
**Researched:** 2026-04-09
**Scope:** NEW capabilities only (vault billing, mesh-llm, Docker deployment, landing page)

## What Already Exists (DO NOT CHANGE)

The core Rust stack is validated through 7 shipped milestones and 244 tests. No changes needed to:
- Tokio/axum/reqwest/serde/sqlx/clap/tracing/secrecy/dashmap/thiserror
- See Cargo.toml for pinned versions

## New Stack Additions

### Vault Billing Integration

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| *No new crates* | - | VaultClient already in `src/proxy/vault.rs` | reqwest handles all HTTP to vault. Reserve/settle/release pattern is fully implemented with retry, backoff, and pending settlement persistence. The code exists, it just needs to be wired into the handler hot path with real agent tokens. |

**What's needed is wiring, not new dependencies.** The VaultClient already:
- Makes HTTP POST calls to `/internal/reserve`, `/internal/settle`, `/internal/release`
- Uses `X-Internal-Token` header auth
- Has retry with exponential backoff (3 retries, 100ms base)
- Persists pending settlements to SQLite for crash recovery
- Runs a background reconciliation loop
- Estimates reserve amounts via `estimate_reserve_msats()`

**Integration points to implement:**
1. Extract bearer token from client request as `agent_token` (forwarded to vault for agent identification)
2. Call `reserve()` before routing, fail with 402/403 if vault denies
3. Call `settle()` with actual cost after successful inference
4. Call `release()` on inference failure (refund buyer)
5. Handle vault-unavailable gracefully (pending settlement queue already exists)
6. Wire backpressure flag -- reject new requests when pending settlements exceed threshold

**Confidence:** HIGH -- vault.rs is 600 lines of working code with unit tests.

### mesh-llm Provider Support

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| *No new crates* | - | mesh-llm exposes standard OpenAI-compatible API on localhost:9337 | arbstr already routes to any OpenAI-compatible endpoint. mesh-llm is just another `[[providers]]` entry in config.toml. |

**mesh-llm is architecturally identical to any other provider.** It exposes:
- `POST /v1/chat/completions` -- standard OpenAI chat completions
- `GET /v1/models` -- list available models (Qwen3-32B, GLM-4.7-Flash, Llama variants, etc.)
- No API key required (local P2P network, no auth)
- Default port: 9337
- Supports streaming via SSE (same as all OpenAI-compatible endpoints)

**What makes mesh-llm special (config-level, not code-level):**
- `tier = "local"` -- already supported in v1.7's tier system
- `url = "http://localhost:9337/v1"` or `http://host.docker.internal:9337/v1` in Docker
- No `api_key` field needed (convention-based lookup finds nothing, falls through cleanly)
- Pricing: effectively free (your own compute), so `input_rate = 0`, `output_rate = 0`, `base_fee = 0`

**Example config entry:**
```toml
[[providers]]
name = "mesh-local"
url = "http://localhost:9337/v1"
models = ["Qwen3-32B"]
tier = "local"
input_rate = 0
output_rate = 0
```

**Future auto-discovery** (NOT v2.0): mesh-llm exposes available models via `/v1/models`. A future feature could poll this endpoint and auto-register models. For v2.0, manual config is sufficient and consistent with how all other providers are configured.

**Confidence:** HIGH -- mesh-llm docs confirm standard OpenAI-compatible API at localhost:9337.

### Docker Multi-Service Deployment

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| Docker Compose | v2 spec | Multi-service orchestration | Already scaffolded in arbstr-node repo with 4 services (core, vault, LND, mint). Uses `depends_on` with healthcheck conditions for startup ordering. |
| lightninglabs/lnd | **v0.20.1-beta** | Lightning Network daemon | Current scaffold uses v0.18.4-beta (2024). v0.20.1-beta (Feb 2026) is latest stable release. Upgrade for security patches and performance improvements. |
| cashubtc/nutshell | **0.20.0** | Cashu ecash mint | Current scaffold uses 0.16.3. v0.20.0 is latest release. Upgrade for NUT protocol improvements. |
| rust:1.86-slim | - | Builder image for Rust compilation | Multi-stage build: compile in rust:1.86-slim, copy binary to debian:bookworm-slim runtime. |
| debian:bookworm-slim | - | Runtime image for arbstr core | ~80MB. Includes glibc needed by sqlx/reqwest OpenSSL. curl for healthchecks. |

**Dockerfile for arbstr core (new file needed):**

```dockerfile
# Stage 1: Build
FROM rust:1.86-slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
ENV SQLX_OFFLINE=true
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/arbstr /usr/local/bin/arbstr
ENTRYPOINT ["arbstr"]
CMD ["serve", "-c", "/config/config.toml"]
```

**Key Docker decisions:**

| Decision | Rationale |
|----------|-----------|
| debian:bookworm-slim over Alpine | sqlx and reqwest depend on OpenSSL (dynamically linked). Alpine uses musl which requires cross-compilation setup and static linking. Not worth the complexity for ~30MB size savings on a server image. |
| SQLX_OFFLINE=true | Avoids needing a live SQLite database during Docker build. Requires `cargo sqlx prepare` run beforehand to generate `.sqlx/` metadata directory (commit to repo). |
| curl in runtime image | Needed for Docker healthcheck commands (`curl -f http://localhost:8080/health`). |
| host.docker.internal for mesh-llm | mesh-llm runs on the host, not in Docker. Core container accesses it via `host.docker.internal:9337`. Requires `extra_hosts` in compose. |

**arbstr-node repo updates needed:**
1. Update LND image tag: `v0.18.4-beta` -> `v0.20.1-beta`
2. Update Nutshell image tag: `0.16.3` -> `0.20.0`
3. Switch core service from `image: arbstr/core:latest` to `build: context` pointing at arbstr repo (or git submodule)
4. Switch vault service from `image: arbstr/vault:latest` to `build: context` pointing at vault repo
5. Add `extra_hosts: ["host.docker.internal:host-gateway"]` to core service for mesh-llm on host
6. Remove scaffold TODO comments (vault /internal/* routes now exist)

**Confidence:** MEDIUM for image version upgrades (verify against Docker Hub tags before pinning). HIGH for Dockerfile pattern and compose architecture.

### Landing Page (arbstr.com)

| Technology | Version | Purpose | Why |
|------------|---------|---------|-----|
| Plain HTML + CSS | - | Static landing page | A single-page marketing site does not need a static site generator, build toolchain, or JavaScript framework. One `index.html` with linked CSS loads instantly, has zero dependencies, and is trivially deployable. |
| GitHub Pages | - | Hosting | Free, automatic HTTPS via Let's Encrypt, custom domain support (`CNAME` file), deploys on push to main branch. Zero infrastructure to manage. |

**Why NOT a static site generator:**
- Zola, Hugo, Jekyll all add toolchain complexity for a single page
- No blog, no multiple pages, no templating, no content management needed
- A landing page is one HTML file with CSS -- adding a build step is over-engineering
- If the site grows beyond one page later, migrate to Zola then

**Landing page structure:**
```
arbstr.com/          # Separate repo (github.com/johnzilla/arbstr.com) or gh-pages branch
  index.html         # Single page: hero, value prop, getting started, footer
  style.css          # Separate stylesheet
  favicon.ico
  CNAME              # GitHub Pages custom domain: arbstr.com
```

**Confidence:** HIGH -- plain HTML on GitHub Pages is the simplest correct answer for a single-page landing site.

## Alternatives Considered

| Category | Recommended | Alternative | Why Not |
|----------|-------------|-------------|---------|
| Vault HTTP client | reqwest (existing) | hyper directly | reqwest is already in Cargo.toml with JSON and streaming support. hyper is lower-level for no benefit. |
| mesh-llm integration | Config-only provider entry | Custom mesh-llm discovery crate | mesh-llm is OpenAI-compatible. Writing a custom client would duplicate reqwest logic already in the router. Auto-discovery is future scope. |
| Docker base image | debian:bookworm-slim | Alpine Linux | sqlx/reqwest need OpenSSL (glibc). Alpine uses musl -- cross-compilation adds build complexity for minimal size savings. |
| Docker base image | debian:bookworm-slim | distroless (gcr.io/distroless/cc) | Distroless lacks curl for healthchecks and shell for debugging. Not worth the tradeoff for an internal service. |
| Landing page | Plain HTML + GitHub Pages | Zola / Hugo | Single page does not need a static site generator. |
| Landing page | Plain HTML + GitHub Pages | Next.js / Astro | JavaScript frameworks for a static page with no interactivity is unnecessary complexity. |
| Landing page hosting | GitHub Pages | Cloudflare Pages | Both free. GitHub Pages is simpler for a GitHub-hosted project. Cloudflare has better global CDN but irrelevant for a landing page. |

## What NOT to Add

| Don't Add | Why |
|-----------|-----|
| gRPC crate (tonic) | Vault uses HTTP/JSON, not gRPC. LND gRPC is accessed by vault service, not core. |
| WebSocket crate | mesh-llm uses standard HTTP/SSE streaming, same as all other providers. Already handled by reqwest + tokio-stream. |
| Service mesh (Consul, Envoy) | Docker Compose `depends_on` with healthchecks handles startup ordering for 4 services. |
| Kubernetes manifests | Docker Compose is the deployment target. K8s is future scope if multi-node federation happens. |
| Frontend framework (React, Vue, Svelte) | Landing page is static HTML. No interactivity needed. |
| Database migration tool | sqlx embedded migrations already handle schema changes. |
| Monitoring stack (Prometheus, Grafana) | Out of scope for v2.0. `/v1/stats` and `/health` endpoints are sufficient. |
| API gateway (Kong, Traefik) | arbstr IS the gateway. Adding another reverse proxy is redundant. |
| mesh-llm SDK/client library | Does not exist and is not needed. Standard OpenAI-compatible HTTP works. |
| Cashu client crate | Cashu payments are handled by the vault service (TypeScript), not core (Rust). |

## Pre-Build Requirements

Before Docker image build, generate offline query metadata:

```bash
# In arbstr repo root
cargo sqlx prepare --workspace
# Creates .sqlx/ directory -- commit it to the repo
# Required because Docker build has no live SQLite database
```

## New Files Summary

```
# In arbstr repo (NEW)
Dockerfile                    # Multi-stage Rust build
.sqlx/                        # Generated by cargo sqlx prepare (commit this)

# In arbstr-node repo (MODIFY)
docker-compose.yml            # Update image versions, switch to build contexts
config.toml                   # Uncomment mesh-llm provider example

# Landing page (NEW repo or branch)
arbstr.com/
  index.html                  # Single page
  style.css                   # Styles
  CNAME                       # GitHub Pages domain: arbstr.com
```

**Zero new Cargo dependencies.** The entire v2.0 milestone ships with the same Cargo.toml.

## Sources

- [mesh-llm documentation](https://docs.anarchai.org/) -- OpenAI-compatible API at localhost:9337, model catalog (HIGH confidence)
- [mesh-llm GitHub](https://github.com/michaelneale/mesh-llm) -- reference implementation, P2P architecture (HIGH confidence)
- [LND releases](https://github.com/lightningnetwork/lnd/releases) -- v0.20.1-beta released Feb 2026 (MEDIUM confidence -- verify Docker Hub tag)
- [lightninglabs/lnd Docker Hub](https://hub.docker.com/r/lightninglabs/lnd) -- official Docker images (MEDIUM confidence)
- [Nutshell (Cashu) releases](https://github.com/cashubtc/nutshell/releases) -- v0.20.0 (MEDIUM confidence -- verify Docker Hub tag)
- [realworld-axum-sqlx](https://github.com/launchbadge/realworld-axum-sqlx) -- Dockerfile patterns for axum+sqlx (HIGH confidence)
- arbstr `src/proxy/vault.rs` -- existing VaultClient implementation, 600 lines with tests (HIGH confidence)
- arbstr `Cargo.toml` -- current dependency versions (HIGH confidence)
- arbstr-node `docker-compose.yml` -- existing 4-service scaffold (HIGH confidence)
