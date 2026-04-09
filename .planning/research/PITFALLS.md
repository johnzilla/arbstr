# Pitfalls Research

**Domain:** Adding vault billing, mesh-llm provider, Docker deployment, and landing page to an existing Rust LLM proxy
**Researched:** 2026-04-09
**Confidence:** HIGH (based on direct codebase analysis of vault.rs, handlers.rs, stream.rs, docker-compose.yml, config.toml; mesh-llm API docs at docs.anarchai.org; Cashu protocol documentation; distributed billing pattern literature)

## Critical Pitfalls

### Pitfall 1: Reserve Uses Cheapest Candidate Rates but Tier Escalation Routes to Expensive Provider

**What goes wrong:** The vault reserve call on line 623-632 of handlers.rs estimates cost using `resolved.candidates[0]` (cheapest candidate). But after reserve, if the cheapest provider circuit-breaks during retry, fallback routes to a more expensive provider. The actual cost exceeds the reserved amount. The vault settle call sends `actual_msats > reserved_msats`, which the vault must either reject (losing the provider payment) or silently accept (creating an unbacked liability).

**Why it happens:** Reserve happens before routing, but actual provider selection happens during retry/fallback. Tier escalation (line 300-312 of handlers.rs) further compounds this: a request scored as "local" tier can escalate to "standard" or "frontier" providers with 3-10x higher rates, but the reservation was calculated at local rates.

**Consequences:** Double-billing if vault rejects over-reserve settles, or treasury losses if vault silently accepts. At scale, systematic under-reservation drains the treasury.

**Prevention:** Reserve at worst-case rates, not cheapest. Use the most expensive candidate across all possible escalation tiers for the reservation estimate. The `estimate_reserve_msats` function should accept the highest `output_rate` across all fallback candidates. If the reserve amount is based on the cheapest and settle exceeds it, the vault must have explicit "over-settle" handling (reject and queue for manual review, or accept with an alert). The PROJECT.md already flags this: "Vault reservation under tier escalation needs frontier-tier pricing (worst case)."

**Detection:** Monitor `settled_msats > reserved_msats` ratio in vault logs. Alert if this exceeds 1%.

### Pitfall 2: Streaming Settle/Release Race with Client Disconnect

**What goes wrong:** The streaming path (lines 1364-1445 of handlers.rs) runs vault settle/release in the post-stream background task. If the client disconnects mid-stream, the handler design correctly continues consuming upstream to extract usage data. But there is a window where: (a) the stream task panics (despite catch_unwind), (b) the tokio task is cancelled during graceful shutdown, or (c) the process crashes between provider response completion and vault settle. In all three cases, a reservation is left permanently open -- funds locked but never settled or released.

**Why it happens:** The settle/release is fire-and-forget via spawn. If the spawn task never completes, the pending_settlements table never gets the record, and reconciliation has nothing to replay.

**Consequences:** Customer funds permanently locked in reservations. Vault balance slowly drains as reservations accumulate without settlement.

**Prevention:** Write the pending settlement record to SQLite BEFORE attempting the vault API call, then delete it on success. This inverts the current pattern (which only writes to pending_settlements on vault API failure). The reconciliation loop then handles all incomplete settlements, including ones where the process crashed before the vault call. This is the "write-ahead log" pattern.

**Detection:** Vault should have a reservation expiry (e.g., 10 minutes). Reservations not settled or released within the window auto-release with an alert. Monitor reservation age distribution.

### Pitfall 3: Cashu Token Double-Spend During Provider Retry

**What goes wrong:** When a provider fails and arbstr retries on a different provider, the first provider may have already consumed compute (partially generated tokens) but the settle was released. If Routstr providers accept Cashu tokens as payment (via api_key), and the retry sends the same Cashu token to a second provider, the second provider rejects it as already spent. The request fails on retry even though the token was not actually consumed by the first provider.

**Why it happens:** Cashu ecash tokens are bearer instruments. Once presented to a mint for redemption, the proofs are burned. If a provider redeems the token proof during request processing (before completing inference), the same proof cannot be reused. This is distinct from the vault billing path -- it applies when providers accept direct Cashu payment rather than going through the vault.

**Consequences:** Retry fails with payment error. User loses funds on failed first attempt AND cannot retry. Silent failure mode if error is swallowed.

**Prevention:** For vault-mediated billing (the v2.0 path), this is not an issue since vault handles settlement, not raw Cashu tokens. Ensure that the provider `api_key` path (direct Cashu tokens to Routstr) is clearly separated from the vault billing path. When vault billing is active, provider api_keys should be service-level credentials, not per-request Cashu tokens. Document this distinction explicitly.

**Detection:** Log provider rejection reasons. Alert on "token already spent" or "proof already used" errors from providers.

### Pitfall 4: Vault Down During Startup Blocks All Requests Indefinitely

**What goes wrong:** The docker-compose.yml has `core` depending on `vault` with `condition: service_healthy`. If vault's health check passes (HTTP 200 on /health) but the vault database migration is still running, or LND is in a degraded state, core starts and immediately begins accepting requests. The first vault.reserve() call hangs or returns 500. All requests fail until vault is truly ready. Conversely, if vault never becomes healthy, core never starts -- no free proxy mode fallback.

**Why it happens:** Health check endpoint may return 200 before all internal subsystems are ready (database, LND connection, Cashu mint connection). The 15-second `start_period` in the compose file may not be enough for LND to fully sync even in regtest mode.

**Consequences:** Users see 503 errors for the first N seconds after deployment. Or, if vault stays unhealthy, the entire node is down even though core could serve requests in free proxy mode.

**Prevention:** (1) Vault health check should verify database connectivity AND LND connectivity, not just HTTP liveness. (2) Core should have a grace period where vault unavailability falls back to free proxy mode with warnings, rather than hard-failing. (3) Add a `vault_required: bool` config option -- when false, vault being down degrades to free proxy mode instead of blocking.

**Detection:** Monitor time-to-first-successful-request after deployment. Alert if > 30 seconds.

## Moderate Pitfalls

### Pitfall 5: mesh-llm Model Names Mismatch Provider Config

**What goes wrong:** mesh-llm uses fuzzy model name matching internally (e.g., `Qwen3-8B` matches `Qwen3-8B-Q4_K_M`). But arbstr's provider config requires exact model names in the `models = [...]` array. If the mesh-llm node loads a quantized variant (Q4_K_M suffix), and the config says `models = ["Qwen3-8B"]`, the router will never match requests for "Qwen3-8B" to the mesh-llm provider because the /v1/models endpoint returns the full quantized name.

**Prevention:** For mesh-llm provider type, implement model name normalization or prefix matching. When a provider is configured as `type = "mesh-llm"`, auto-discover models via GET /v1/models on startup and register all returned model names. Config `models = [...]` becomes optional for mesh-llm providers.

### Pitfall 6: mesh-llm Node Disappears Mid-Inference

**What goes wrong:** mesh-llm is a P2P network where nodes can disconnect at any time. Dead hosts are detected via 60-second heartbeat, but mid-inference, if a peer contributing to a distributed model goes down, the inference fails partway through. For streaming requests, this means a partial SSE stream followed by silence -- no `[DONE]` marker, no usage data.

**Why it happens:** mesh-llm distributes MoE expert layers across peers. If a peer handling specific experts disconnects during token generation, the inference pipeline stalls. mesh-llm itself is "work in progress -- use with caution" per its docs.

**Consequences:** The SSE observer waits for `[DONE]` with a strict requirement (line noted in stream.rs). Without it, `StreamResult` returns empty -- no usage data, so cost is unknown. The vault settle path (line 1420 of handlers.rs) then does a full release ("stream_incomplete_no_usage") -- correct behavior, but the user got partial output and lost their reservation hold time.

**Prevention:** (1) Configure mesh-llm providers with shorter circuit breaker timeouts than cloud providers. (2) For mesh-llm type providers, consider a "last known good" model list that persists across restarts (don't re-discover models that just went offline). (3) mesh-llm providers should default to `tier = "local"` so they are tried first (cheapest) but escalation to cloud providers works as fallback.

### Pitfall 7: Docker Volume Permissions for SQLite

**What goes wrong:** The core service writes to `/data/arbstr.db` via a Docker volume. If the Rust binary runs as a non-root user (security best practice) but the volume is created by Docker with root ownership, SQLite writes fail with "unable to open database file" or "read-only database." WAL mode requires write access to the directory containing the database (for the `-wal` and `-shm` files), not just the file itself.

**Prevention:** (1) In the Dockerfile, create the data directory and `chown` it to the application user before switching to non-root. (2) Alternatively, use a named volume with explicit uid/gid mapping. (3) Test with `docker compose up` from a clean state (no pre-existing volumes) as part of CI.

### Pitfall 8: LND Wallet Not Initialized in Regtest

**What goes wrong:** The docker-compose uses `--noseedbackup` for LND which auto-creates a wallet. But in regtest mode, there are no blocks and no funds. The Cashu mint configured with `FakeWallet` backend works without real Lightning, but if/when the deployment switches to real Lightning (even regtest), LND needs funded channels. The vault tries to pay invoices via LND, which fails with "insufficient balance" at the Lightning level, distinct from the agent-level balance.

**Prevention:** (1) Document clearly that the default docker-compose is development-only with FakeWallet. (2) For production, provide a separate compose profile or override file that removes FakeWallet and adds channel funding scripts. (3) Vault should distinguish between "agent has no funds" (402) and "infrastructure payment failure" (503) and return appropriate errors.

### Pitfall 9: Cross-Service Timeout Mismatch (Rust to TypeScript)

**What goes wrong:** The vault client has a 5-second timeout (`VAULT_TIMEOUT` in vault.rs line 28). But the vault (TypeScript/Node.js) itself may need to call LND gRPC (which could take seconds if LND is busy) and then write to its database. If LND is slow (channel negotiation, block processing), the vault exceeds 5 seconds, arbstr gets a timeout, retries, and the vault receives duplicate reserve requests. Since reserves are not idempotent (each creates a new hold), this double-reserves funds.

**Prevention:** (1) Make vault reserve idempotent using `correlation_id` as a deduplication key. If the same correlation_id is reserved twice, return the existing reservation instead of creating a new one. (2) Increase `VAULT_TIMEOUT` to 10 seconds or make it configurable. (3) Vault should have its own timeout for LND calls that is shorter than the arbstr-to-vault timeout, providing a clean error before arbstr times out.

**Detection:** Monitor vault reserve calls where the same correlation_id appears more than once.

### Pitfall 10: Pending Settlement Replay Ordering

**What goes wrong:** The reconciliation loop (vault.rs line 492-558) replays all pending settlements in order. If a settle and release for the same reservation_id both end up in the pending table (due to a race between the streaming path and the timeout path), the reconciler replays both. Depending on order: settle then release = correct (settle wins, release is no-op). Release then settle = vault may reject the settle because reservation was already released.

**Prevention:** (1) Use `INSERT OR IGNORE` with UNIQUE on reservation_id (already done -- line 350). But the current schema allows both a settle AND a release for the same reservation_id since the UNIQUE constraint is on reservation_id alone, and there can be one settle row and one release row if the settlement_type differs. (2) Add a check: before inserting a pending settlement, delete any existing pending record for the same reservation_id. Last write wins is correct because the latest state is the most accurate.

### Pitfall 11: Bearer Token Dual Purpose (Auth vs Agent ID)

**What goes wrong:** When vault billing is active, the handler extracts the Authorization Bearer token and passes it as `agent_token` to `vault.reserve()`. But this same token may also be the `auth_token` configured in `[server]` for basic proxy authentication. If the server auth_token and the agent's vault token are different (they should be -- server auth protects the proxy, vault token identifies the billing agent), the server auth middleware may reject valid agent tokens, or the vault receives the proxy auth token instead of the agent token.

**Prevention:** (1) Clearly separate concerns: server `auth_token` is for proxy access control; vault `agent_token` is for billing identity. (2) If both are active, the server auth middleware should validate against its own token list, and the vault billing path should forward the original client token without modification. (3) Consider using different headers (e.g., X-Arbstr-Agent-Token for vault identity) to avoid collision, or document that when vault billing is enabled, the Bearer token is the agent token and server auth_token should be disabled.

## Minor Pitfalls

### Pitfall 12: Landing Page SEO with SPA Framework

**What goes wrong:** Building arbstr.com as a React/Next.js SPA when it is fundamentally a static marketing page. Search engines may not index JavaScript-rendered content properly, page load is slow for a page that should be instant, and the build toolchain is disproportionately complex for what is essentially HTML + CSS.

**Prevention:** Use a static site generator (Astro, Hugo) or plain HTML/CSS. The landing page has no dynamic content, no user authentication, no API calls. Static HTML serves from any CDN with zero JavaScript, sub-second load times, and perfect SEO. Deploy to GitHub Pages or Cloudflare Pages for free hosting.

### Pitfall 13: Docker Image Size for Rust Binary

**What goes wrong:** A naive `FROM rust:latest` Dockerfile produces a 1.5GB+ image. The Rust compiler toolchain, all crate source, and build artifacts are included in the final image. This slows down pulls, increases attack surface, and wastes disk.

**Prevention:** Multi-stage build: compile in `rust:1.XX-bookworm`, copy the binary to `debian:bookworm-slim` or `gcr.io/distroless/cc-debian12`. Final image should be under 50MB. Pin the Rust version to match the project's `rust-toolchain.toml` or MSRV.

### Pitfall 14: mesh-llm Provider Behind Docker NAT

**What goes wrong:** mesh-llm runs on the host machine at `localhost:9337`. The Docker container for arbstr core cannot reach `localhost:9337` because localhost inside the container refers to the container itself. The config.toml in arbstr-node uses `http://host.docker.internal:9337/v1` but `host.docker.internal` only works on Docker Desktop (macOS/Windows), not on native Linux Docker.

**Prevention:** On Linux, use `network_mode: host` for the core container (sacrifices network isolation), or use `extra_hosts: ["host.docker.internal:host-gateway"]` (Docker 20.10+). Document this in the arbstr-node README. Test on Linux specifically since most deployment targets are Linux.

### Pitfall 15: Graceful Shutdown Ordering with Vault

**What goes wrong:** On SIGTERM, arbstr core runs graceful shutdown: stop accepting requests, drain in-flight, then exit. But if vault settle calls are in-flight (fire-and-forget spawned tasks), the process may exit before settles complete. The reconciliation loop does a final `reconcile_once` (line 476), but spawned settle/release tasks are not tracked and may be cancelled by the runtime shutting down.

**Prevention:** Track all spawned vault tasks in a JoinSet. During graceful shutdown, await all tasks in the JoinSet before exiting. This ensures all settle/release calls complete (or fail and write to pending_settlements) before the process exits.

## Phase-Specific Warnings

| Phase Topic | Likely Pitfall | Mitigation |
|-------------|---------------|------------|
| Vault billing integration | Reserve at cheapest rate, settle at higher rate after escalation (Pitfall 1) | Reserve at worst-case (frontier) rates across all candidates |
| Vault billing integration | Reservation leak on process crash (Pitfall 2) | Write-ahead pending settlement before vault call; vault reservation expiry |
| Vault billing integration | Double-reserve on timeout/retry (Pitfall 9) | Make reserve idempotent on correlation_id |
| Vault billing integration | Bearer token purpose collision (Pitfall 11) | Separate auth_token from agent_token concerns |
| mesh-llm provider | Model name mismatch with quantized variants (Pitfall 5) | Auto-discover models via /v1/models endpoint |
| mesh-llm provider | Mid-inference node failure (Pitfall 6) | Shorter circuit breaker, local tier default, cloud fallback |
| mesh-llm provider | Cannot reach host from Docker container on Linux (Pitfall 14) | Use host-gateway extra_hosts directive |
| Docker deployment | Volume permissions break SQLite (Pitfall 7) | chown in Dockerfile, test from clean volumes |
| Docker deployment | Vault not ready despite health check (Pitfall 4) | Deep health check, graceful degradation to free proxy |
| Docker deployment | LND not funded in regtest (Pitfall 8) | Document dev vs prod clearly, FakeWallet for dev |
| Docker deployment | Rust image bloat (Pitfall 13) | Multi-stage build, distroless runtime |
| Landing page | SPA over-engineering (Pitfall 12) | Static HTML/CSS, deploy to CDN |
| Graceful shutdown | Spawned vault tasks cancelled (Pitfall 15) | Track tasks in JoinSet, await before exit |

## Sources

- Direct codebase analysis: `src/proxy/vault.rs`, `src/proxy/handlers.rs`, `src/proxy/stream.rs`, `docker-compose.yml`
- [mesh-llm documentation](https://docs.anarchai.org/) -- API, discovery, node failure semantics
- [mesh-llm GitHub](https://github.com/michaelneale/mesh-llm) -- "work in progress" status, fuzzy model matching
- [Cashu protocol docs](https://docs.cashu.space/protocol) -- double-spend prevention via proof burning
- [Cashu eNuts double-spend issue](https://github.com/cashubtc/eNuts/issues/295) -- token redemption semantics
- [Docker Compose health checks](https://docs.docker.com/reference/compose-file/services/#healthcheck) -- service_healthy condition
- [Rust Docker containerization](https://oneuptime.com/blog/post/2026-02-01-rust-docker-containerization/view) -- multi-stage build patterns
- PROJECT.md known concerns: "Vault reservation under tier escalation needs frontier-tier pricing (worst case)"
- PROJECT.md known concerns: "Streaming errors are silent (no retry for streaming)"
