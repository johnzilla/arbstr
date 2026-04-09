# Architecture Patterns

**Domain:** Inference marketplace (LLM routing + Bitcoin billing + Docker deployment)
**Researched:** 2026-04-09

## Current Architecture (Baseline)

```
                           +-----------+
   OpenAI clients -------->| arbstr    |--------> Routstr / remote providers
   (Claude Code,           | core      |--------> mesh-llm (localhost:9337)
    Cursor, SDKs)          | (Rust)    |--------> Ollama / any OpenAI-compat
                           +-----+-----+
                                 |
                          +------+------+
                          |   SQLite    |
                          | (requests,  |
                          |  pending    |
                          |  settlements|
                          +-------------+
```

Core is a single Rust binary (`axum` server) with:
- `proxy/handlers.rs` -- request entry, vault billing hooks, circuit breaker integration
- `proxy/vault.rs` -- `VaultClient` with reserve/settle/release over HTTP, pending settlement persistence, reconciliation loop
- `router/selector.rs` -- cheapest-first provider selection with tier awareness (local/standard/frontier)
- `router/complexity.rs` -- heuristic complexity scoring (5 weighted signals)
- `storage/` -- bounded channel writer to SQLite

Vault is a separate TypeScript/Fastify service (github.com/johnzilla/arbstr-vault) with:
- `/internal/reserve`, `/internal/settle`, `/internal/release` endpoints
- Agent sub-accounts with RESERVE/RELEASE/PAYMENT ledger
- Cashu ecash for micropayments, Lightning for larger settlements
- `X-Internal-Token` auth between core and vault

## Recommended Architecture (v2.0)

### System Topology

```
 docker-compose.yml (arbstr-node)
 +---------------------------------------------------------+
 |                                                         |
 |  +----------+    HTTP     +----------+                  |
 |  | core     |----------->| vault    |                  |
 |  | :8080    |<-----------| :3000    |                  |
 |  +----+-----+            +----+-----+                  |
 |       |                       |                         |
 |       |                  +----+-----+    +----------+  |
 |       |                  | lnd      |    | mint     |  |
 |       |                  | :10009   |    | :3338    |  |
 |       |                  +----------+    +----------+  |
 |       |                                                 |
 +-------|------ Docker bridge network -------------------+
         |
         | host.docker.internal (or host network)
         v
   +----------+
   | mesh-llm |  (runs on host, not in Docker)
   | :9337    |
   +----------+
```

### Component Boundaries

| Component | Responsibility | Communicates With | New vs Modified |
|-----------|---------------|-------------------|-----------------|
| **core** (Rust) | Request routing, vault billing, circuit breaking, complexity scoring, SSE streaming | vault (HTTP), providers (HTTP), SQLite (embedded) | MODIFIED -- Dockerfile, agent token passthrough, end-to-end vault testing |
| **vault** (TypeScript) | Agent accounts, balance management, Cashu/Lightning settlement, ledger | lnd (gRPC), mint (HTTP), core (responds to HTTP) | EXISTING -- needs Dockerfile in arbstr-vault repo |
| **lnd** | Lightning Network daemon for payment settlement | vault (gRPC) | EXISTING -- upstream image `lightninglabs/lnd:v0.18.4-beta` |
| **mint** | Cashu mint for ecash micropayments | vault (HTTP) | EXISTING -- upstream image `cashubtc/nutshell:0.16.3` |
| **mesh-llm** | Distributed P2P inference, model serving | core (receives HTTP from core) | NOT CONTAINERIZED -- runs on host |
| **landing page** | Marketing site, anti-token manifesto | None (static) | NEW -- separate concern, not in Docker stack |

### Data Flow

#### Request Flow with Live Vault Billing

```
1. Client POST /v1/chat/completions (Bearer token = agent token)
2. Core extracts agent token from Authorization header
3. Core scores complexity -> selects tier -> picks cheapest candidate
4. Core calls vault POST /internal/reserve
   - Sends: agent_token, estimated_msats, correlation_id, model
   - Receives: reservation_id
   - On 402: return 402 to client (insufficient balance)
5. Core forwards request to provider (mesh-llm, Routstr, etc.)
6. Provider returns response (streaming or non-streaming)
7. Core extracts token usage from response
8. Core calls vault POST /internal/settle
   - Sends: reservation_id, actual_msats, metadata
   - On failure: persists to pending_settlements, reconciliation loop retries
9. Core returns response to client with arbstr headers
```

This flow is already coded in `handlers.rs` and `vault.rs`. The `reserve()` method in `vault.rs` already accepts `agent_token: &str`. The critical integration work is:

1. **Agent token extraction:** The handler must pull the bearer token from the incoming `Authorization` header and pass it to `vault.reserve()`. Currently, `auth_token` in server config is used for core-level auth. When vault is active, the client's bearer token serves dual purpose -- authenticating with core AND identifying the agent in vault.
2. **End-to-end testing:** All vault paths (reserve success, insufficient balance, settle failure with pending persistence, reconciliation replay) need testing against a real vault instance, not just unit tests.

#### Docker Network Communication

Core-to-vault uses Docker Compose's default bridge network. The existing `config.toml` in arbstr-node already has the correct service-name-based URL:

```toml
[vault]
url = "http://vault:3000"
```

Docker Compose creates a shared network where services resolve each other by service name. No extra network configuration needed. The `depends_on` with `condition: service_healthy` in docker-compose.yml ensures vault is ready before core starts. The dependency chain is: `lnd + mint -> vault -> core`.

#### mesh-llm Provider Access from Docker

mesh-llm runs on the host machine (not containerized -- it needs direct GPU access and manages its own P2P networking via Nostr discovery). Core running inside Docker needs to reach `localhost:9337` on the host.

**Use `host.docker.internal` (recommended):**

```toml
[[providers]]
name = "mesh-local"
url = "http://host.docker.internal:9337/v1"
models = ["Qwen3-32B"]
tier = "local"
input_rate = 0
output_rate = 0
```

The arbstr-node docker-compose.yml needs one addition to the core service:

```yaml
core:
  extra_hosts:
    - "host.docker.internal:host-gateway"
```

This works on Linux (Docker Engine) and is automatic on Docker Desktop (macOS/Windows). The `host-gateway` special name maps to the host's gateway IP.

## New Components Detail

### 1. Dockerfile for arbstr core

Multi-stage build. Builder stage compiles Rust; runtime stage copies the binary into a minimal image.

```dockerfile
# Stage 1: Build
FROM rust:1.83-slim-bookworm AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY migrations/ migrations/
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false arbstr
COPY --from=builder /app/target/release/arbstr /usr/local/bin/arbstr
USER arbstr
EXPOSE 8080
CMD ["arbstr", "serve", "-c", "/config/config.toml"]
```

Key decisions:
- `debian:bookworm-slim` over `alpine` because sqlx with SQLite links against glibc. Musl builds require additional work (static linking, cross-compilation) for no meaningful size benefit at this stage.
- `curl` included for the healthcheck command in docker-compose.yml.
- Non-root `arbstr` user for security.
- Config mounted as read-only volume (not baked into image) so the same image works across environments.
- Expected image size: ~80-100MB.

### 2. mesh-llm as Provider Type

mesh-llm is already OpenAI-compatible on `localhost:9337/v1` (confirmed via docs.anarchai.org). It fits directly into the existing `[[providers]]` config with **zero code changes** to the routing engine. Everything needed already exists:

**Health checking:** The circuit breaker (`circuit_breaker.rs`) already handles mesh-llm -- 3 consecutive failures open the circuit, half-open probe after 30s. No new code.

**Model listing:** mesh-llm serves whatever models its mesh has loaded. The `models` list in config must be manually specified for v2.0. Auto-discovery (calling `GET /v1/models` at startup) is a useful enhancement but not required.

**No API key:** mesh-llm on localhost needs no auth. Config supports omitting `api_key` -- the convention-based discovery just silently finds nothing and continues.

**Tier assignment:** `tier = "local"` -- integrates with complexity scorer from v1.7. Simple requests route to mesh-llm; complex ones escalate to Routstr/frontier.

**Zero cost:** `input_rate = 0, output_rate = 0, base_fee = 0`. The cheapest-first selection naturally prefers mesh-llm when the model matches.

### 3. Landing Page (arbstr.com)

Completely decoupled from the arbstr-node Docker stack. Should NOT be a service in docker-compose.yml.

**Hosting:** GitHub Pages or Cloudflare Pages. Static HTML/CSS, no build step.

**Technology:** Plain HTML + CSS, or a minimal static site generator. No React, no SPA. This is marketing, not an application.

## Patterns to Follow

### Pattern 1: Graceful Degradation (Vault Optional)

Vault billing is opt-in. When `[vault]` is omitted from config, core runs in free proxy mode. All vault calls become no-ops via `Option<VaultClient>` in `AppState`.

```rust
// Already in handlers.rs:
if let Some(vault) = &state.vault {
    let reservation = vault.reserve(...).await?;
} else {
    // Free proxy mode, skip billing
}
```

This is critical for v2.0: the same binary works for both free self-hosted use and paid marketplace use. Users can start without vault and add billing later.

### Pattern 2: Docker Service Dependencies with Health Checks

```yaml
core:
  depends_on:
    vault:
      condition: service_healthy
vault:
  depends_on:
    lnd:
      condition: service_healthy
    mint:
      condition: service_healthy
```

Already correct in docker-compose.yml. Creates startup chain: lnd + mint -> vault -> core. Each service has a curl-based healthcheck against its HTTP endpoint.

### Pattern 3: Pending Settlement Persistence

When vault is unreachable during settle/release, the operation persists to `pending_settlements` in SQLite and retries via background `reconciliation_loop`. This is fully implemented in `vault.rs`.

For v2.0, verify this works under real conditions: vault restarts, network blips within Docker, core restarts with pending settlements in the database.

### Pattern 4: Agent Token Passthrough

The client's `Authorization: Bearer <token>` header serves dual purpose:
1. Authenticates with arbstr core (if `auth_token` is configured)
2. Identifies the agent in vault (passed as `agent_token` to reserve)

The `reserve()` method already accepts `agent_token: &str`. The handler needs to extract it from the request and pass it through. This is the main integration wiring needed in `handlers.rs`.

**Design question:** When `auth_token` is set in server config, should the client's bearer token match `auth_token` (core-level auth) AND be forwarded to vault? Or should vault handle all auth? Recommendation: when vault is configured, forward the bearer token to vault and let vault do auth. Core's `auth_token` becomes irrelevant when vault is active (vault validates the agent token). This avoids requiring two different tokens.

## Anti-Patterns to Avoid

### Anti-Pattern 1: Containerizing mesh-llm

**What:** Putting mesh-llm inside docker-compose.yml.

**Why bad:** mesh-llm needs direct GPU access, manages its own P2P networking (Nostr-based mesh discovery, peer joining), and runs as a system-level daemon. Containerizing adds GPU passthrough complexity with no benefit.

**Instead:** Run mesh-llm on the host. Core reaches it via `host.docker.internal:9337`.

### Anti-Pattern 2: Dynamic Provider Discovery at Runtime (for v2.0)

**What:** Core auto-discovers mesh-llm nodes, queries their models, adds them as providers.

**Why bad for v2.0:** Adds complexity, race conditions (mesh-llm changes models mid-request), and makes the system harder to reason about. Static `[[providers]]` config is simple and correct.

**Instead:** Manual config for v2.0. Auto-discovery (Pubky DHT, Nostr relay) is explicitly in the Future roadmap.

### Anti-Pattern 3: Shared SQLite Across Containers

**What:** Mounting the same SQLite file from both core and vault.

**Why bad:** SQLite is not designed for multi-process concurrent access from different containers. WAL mode helps within a single process but does not solve cross-container file locking.

**Instead:** Each service owns its own database. Core has `arbstr.db` (request log, pending settlements). Vault has `vault.db` (agent accounts, ledger). They communicate via HTTP, not shared state.

### Anti-Pattern 4: Landing Page in Docker Stack

**What:** Adding an nginx container to docker-compose.yml for arbstr.com.

**Why bad:** Marketing content has nothing to do with the runtime inference stack. Couples unrelated deployment lifecycles.

**Instead:** GitHub Pages or Cloudflare Pages with separate deployment.

## Integration Points Summary

### Files to CREATE

| File | Location | Purpose |
|------|----------|---------|
| `Dockerfile` | arbstr core repo | Multi-stage Rust build for containerized deployment |
| Landing page | Separate repo or `arbstr.com/` directory | Static marketing site |

### Files to MODIFY

| File | Change | Why |
|------|--------|-----|
| `src/proxy/handlers.rs` | Extract bearer token, pass as `agent_token` to vault reserve; handle auth_token vs vault-token logic | Wire agent authentication for live billing |
| `docker-compose.yml` (arbstr-node) | Add `extra_hosts` for mesh-llm access on core service | Enable core -> mesh-llm communication from Docker |
| `config.toml` (arbstr-node) | Add/uncomment mesh-llm provider entry | Ship with working mesh-llm example |

### Files that NEED NO CHANGES

| File | Why |
|------|-----|
| `src/proxy/vault.rs` | Already implements full reserve/settle/release with retry, pending persistence, and reconciliation |
| `src/proxy/circuit_breaker.rs` | Already handles all provider failures including mesh-llm |
| `src/router/selector.rs` | Already does tier-aware cheapest-first selection |
| `src/router/complexity.rs` | Already scores complexity and maps to tiers |
| `src/config.rs` | Already supports vault config, provider tiers, all needed fields |
| `src/proxy/server.rs` | AppState already has `Option<VaultClient>` |

## Build Order (Dependency-Aware)

Based on component dependencies:

1. **Dockerfile for core** -- No dependencies on other work. Required before Docker-based testing.
2. **Dockerfile for vault** -- Check if arbstr-vault repo already has one; create if not.
3. **Agent token passthrough** -- Modify `handlers.rs` to extract bearer token and pass to vault. Test locally (core + vault both running natively) before Docker.
4. **End-to-end vault billing** -- Test reserve/settle/release flow with real vault. Verify pending settlement persistence and reconciliation.
5. **mesh-llm provider config** -- Zero code changes. Add `extra_hosts` to compose, add provider entry to config. Test with mesh-llm running on host.
6. **Docker integration testing** -- `docker compose up` with all services, verify full flow.
7. **Landing page** -- Completely independent. Can be done in parallel with 1-6.

**Ordering rationale:**
- Dockerfiles first because everything else needs to run in containers for integration testing
- Vault billing before mesh-llm because vault integration is the harder problem with more failure modes
- mesh-llm is trivial (config only) and should be last among infrastructure work
- Landing page is fully parallel, no dependencies

## Sources

- [mesh-llm documentation](https://docs.anarchai.org/) -- API on localhost:9337, Nostr-based discovery, model catalog
- [mesh-llm GitHub](https://github.com/michaelneale/mesh-llm) -- Implementation details, README
- [Docker multi-stage builds](https://docs.docker.com/get-started/docker-concepts/building-images/multi-stage-builds/) -- Official documentation
- Existing arbstr codebase: `vault.rs` (600 lines, full reserve/settle/release), `handlers.rs` (vault integration hooks), `config.rs` (VaultConfig, ProviderConfig with tier), `server.rs` (AppState with Option<VaultClient>)
- Existing arbstr-node repo: `docker-compose.yml` (4 services), `config.toml` (vault URL using Docker service name), `.env.example` (secrets template)
