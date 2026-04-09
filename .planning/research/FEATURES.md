# Feature Landscape

**Domain:** Inference marketplace with Bitcoin settlement (NiceHash for AI)
**Researched:** 2026-04-09
**Context:** Adding vault billing, mesh-llm provider support, Docker Compose deployment, and landing page to existing Rust routing engine with 7 shipped milestones (v1 through v1.7).

## Table Stakes

Features users expect from a billing-enabled inference marketplace. Missing = product feels broken or unusable.

| Feature | Why Expected | Complexity | Dependencies on Existing |
|---------|--------------|------------|--------------------------|
| End-to-end vault billing (reserve/settle/release) | Without real money flow, arbstr is just another free reverse proxy. The entire marketplace value prop requires billing. | Medium | `VaultClient` exists in `vault.rs` with reserve/settle/release methods, retry logic, pending persistence. Handlers need wiring: extract agent token, call reserve before routing, settle on success, release on failure. |
| 402 on insufficient balance | Users must know immediately if they cannot afford a request, not fail mid-stream or silently. | Low | Reserve call already returns 402 from vault. Map `VaultError::InsufficientBalance` to client 402 response. No separate balance-check endpoint needed. |
| Agent token extraction from client | Vault identifies buyers by their bearer token. arbstr must forward the client's Authorization header as `agent_token` to vault's reserve call. | Low | `auth_token` field exists in server config for arbstr's own auth. Client bearer token needs to be passed through to vault as agent identifier. |
| Cost estimation pre-request | Users want to know cost before committing. Standard in paid API services (OpenAI shows pricing, Routstr shows per-token rates). | Low | `POST /v1/cost` already shipped in v1.6. Verify it works with vault pricing and returns accurate estimates. |
| mesh-llm as a configurable provider | mesh-llm is the primary local compute source. Without it, arbstr only routes to paid remote APIs and the "free when local handles it" story breaks. | Low | Provider config already supports any OpenAI-compatible endpoint. mesh-llm exposes OpenAI-compatible API on localhost:9337. Works TODAY by adding a `[[providers]]` entry. The "feature" is documentation and config examples, not code. |
| mesh-llm model list sync | mesh-llm's model catalog changes dynamically (users add/remove GGUF models). Static `models = [...]` in config gets stale immediately. | Medium | Requires `GET /v1/models` polling on mesh-llm at startup or periodically. Current provider config is fully static. Need: model discovery for providers flagged as dynamic. |
| Docker Compose one-command deployment | `docker compose up` must produce a working 4-service stack (lnd + mint + vault + core). Users expect infrastructure-as-code that works on first try. | Medium | docker-compose.yml scaffold exists in arbstr-node with all 4 services defined. Need: working Dockerfiles for core (Rust multi-stage build) and vault (Node.js), verified health check chain, .env.example with real defaults, volume mount testing. |
| Graceful vault-down degradation | If vault goes down mid-operation, the proxy must not crash or hang. Pending settlements must queue for retry. | Low | Already designed: vault is `Option<VaultConfig>` in config, pending settlement table exists with reconciliation loop, backpressure flag at threshold=100. Needs end-to-end testing under failure conditions. |
| Settle/release fault tolerance | Network partitions between core and vault must not lose money. If settle fails, the reservation must persist for later reconciliation. | Low | `pending_settlements` table with `insert_pending_settlement()` and `reconciliation_loop()` already implemented. Needs integration testing: kill vault mid-settle, verify pending row created, restart vault, verify reconciliation replays it. |

## Differentiators

Features that set arbstr apart from other inference proxies and AI marketplaces. Not expected, but create competitive moat.

| Feature | Value Proposition | Complexity | Dependencies on Existing |
|---------|-------------------|------------|--------------------------|
| Anti-token Bitcoin-only settlement | Every competitor (Bittensor/TAO, Render/RNDR, SingularityNET/AGIX) invented governance tokens. arbstr settles in bitcoin via Cashu ecash. No token, no staking, no governance theater. This is the core brand identity. | Low (for v2.0) | Cashu mint and LND already in docker-compose. Settlement layer is architectural (how vault works), not a feature to build. |
| Zero-config local-first routing | mesh-llm on localhost is free. arbstr routes there first (tier=local), only escalates to paid providers when local model cannot handle the complexity. Users pay nothing when their GPU handles it. | Low | Tier-aware routing shipped in v1.7 with complexity scoring and automatic tier escalation on circuit break. Works with mesh-llm as a local-tier provider. |
| Per-request cost transparency in sats | Unlike cloud AI APIs that bill monthly with opaque pricing, every arbstr request shows exact cost in satoshis via response headers and SSE trailing events. Users see what each request costs in real-time. | Low | Already exists: `x-arbstr-cost-sats` header (v1), trailing SSE event with `cost_sats` (v1.2). Verify cost reflects vault settlement amount, not just rate calculation. |
| Buyer deposit via Cashu token | Users paste a Cashu ecash token to fund their account. No KYC, no credit card, no signup form. Bearer instrument = instant onboarding. Unique in the inference marketplace space. | Medium | Vault-side feature: `POST /deposit` accepting Cashu tokens, redeeming against self-hosted mint, crediting agent balance. Cashu mint already runs in docker-compose. |
| Self-hosted sovereignty | Users run their own node, their own mint, their own keys. No platform dependency, no vendor lock-in. Data never leaves their network for local inference. | Low | This is the deployment model (Docker Compose), not a feature to build. Differentiator is positioning and documentation. |
| Landing page with anti-token manifesto | arbstr.com positions the product against web3 AI token projects. Clear ideology attracts Bitcoin-aligned developers and creates viral shareable content. The manifesto IS the marketing. | Medium | No code dependencies. Content creation is the work: what it is, why no tokens, getting started guide, architecture diagram. Should reference NiceHash model (hashpower marketplace but for AI inference). |

## Anti-Features

Features to explicitly NOT build for v2.0. These are scope traps that delay shipping.

| Anti-Feature | Why Avoid | What to Do Instead |
|--------------|-----------|-------------------|
| Web dashboard UI | Adds frontend framework, auth layer, build pipeline. Single-user home network does not need a dashboard. | Keep `/v1/stats` and `/v1/requests` JSON APIs. Users curl or build their own UI. Explicitly in PROJECT.md Out of Scope. |
| Multi-mint Cashu support | Multiple mints means cross-mint trust, token routing, balance aggregation. Massive complexity for zero user benefit at this stage. | Ship with single self-hosted Nutshell mint in docker-compose. Explicitly scoped out in PROJECT.md. |
| Seller/provider account type | Selling compute requires payout flows, Lightning withdrawal, reputation system, dispute resolution. This is an entire product surface. | v2.0 is buyer-only. Users buy inference, providers are configured statically. Seller accounts are in PROJECT.md "Future" list. |
| Cross-node federation | Nodes discovering each other requires DHT/relay infrastructure (Pubky, Nostr). Premature until single-node works perfectly. | Single-node deployment for v2.0. Federation is explicitly in PROJECT.md "Future" requirements. |
| L402 anonymous access | HTTP 402 challenge/response protocol with macaroon minting and stateless auth. Interesting but premature -- need basic bearer auth working first. | Bearer token auth exists. L402 is in PROJECT.md "Future" list. |
| Provider reputation system | Requires Pubky semantic tags, trust scoring, historical performance weighting. | Circuit breakers (v1.4) provide basic reliability signal. Reputation is post-v2.0. |
| Cross-model fallback | Silently substituting a cheaper model changes output quality without user consent. | Explicitly in PROJECT.md Out of Scope. Fail with clear error if requested model is unavailable. |
| Streaming error retry | Cannot replay a stream body. Would require buffering entire response or protocol changes. | Known limitation. Fail fast on stream errors. Streaming bypasses retry (existing design decision). |
| Dynamic provider pricing | Real-time price discovery across providers. Requires price feeds, bid/ask spreads. | Static rates in TOML config. Price changes require config edit and restart. Dynamic pricing is marketplace-scale complexity. |
| Multi-user account management | User registration, authentication, authorization, quotas. | Single-user deployment. The agent_token identifies the user to vault, but vault manages accounts. Core proxy does not manage users. |

## Feature Dependencies

```
Vault Billing (critical path -- everything else is secondary)
  |
  +-- Handler integration: extract agent_token, call reserve, settle/release
  |     |
  |     +-- Non-streaming path: reserve -> route -> settle/release -> respond
  |     |
  |     +-- Streaming path: reserve -> route -> stream -> post-stream settle/release
  |     |
  |     +-- Error mapping: VaultError -> client HTTP status (402/403/429/503)
  |
  +-- Pending settlement persistence (EXISTS -- needs integration test)
  |
  +-- Reconciliation loop (EXISTS -- needs integration test)
  |
  +-- Buyer Cashu deposit (vault-side, DEFERRED -- manual balance seeding for testing)

mesh-llm Provider
  |
  +-- Static config entry (WORKS TODAY -- document it)
  |
  +-- Dynamic model list sync (NEW -- GET /v1/models polling)
  |     |
  |     +-- Provider type field: `type = "mesh"` triggers auto-discovery
  |
  +-- Tier routing (EXISTS -- v1.7 tier=local with complexity scoring)

Docker Compose Deployment
  |
  +-- Vault billing must work (core <-> vault integration tested)
  |
  +-- Dockerfile for core (Rust multi-stage build)
  |
  +-- Dockerfile for vault (Node.js)
  |
  +-- Health check chain: lnd -> mint -> vault -> core
  |
  +-- .env.example with documented secrets
  |
  +-- Config template with mesh-llm + routstr examples

Landing Page (arbstr.com)
  |
  +-- No code dependencies on the above
  |
  +-- Write AFTER features work (content must describe what actually shipped)
  |
  +-- Sections: hero, anti-token manifesto, how it works, getting started, architecture
```

## MVP Recommendation

### Prioritize (in dependency order):

1. **Vault billing in handlers** -- Wire `VaultClient` into request handlers. Extract agent token from client Authorization header, call reserve before routing, settle on success, release on failure. Handle both streaming and non-streaming paths. Map vault errors to client HTTP status codes. This is THE critical feature -- without it, v2.0 has no marketplace.

2. **mesh-llm static provider config** -- Document how to add mesh-llm as a `[[providers]]` entry with `tier = "local"` and `url = "http://localhost:9337/v1"`. This works today with zero code changes. Write config example, test against live mesh-llm instance.

3. **Docker Compose hardening** -- Create Dockerfiles for core and vault, verify health check chain, test full `docker compose up` end-to-end. The scaffold in arbstr-node exists; make it actually boot and serve requests.

4. **Vault integration testing** -- Test fault tolerance: kill vault mid-settle, verify pending settlement persistence, restart vault, verify reconciliation replays. Test insufficient balance (402). Test backpressure activation.

5. **mesh-llm dynamic model discovery** -- Add periodic `/v1/models` polling for providers with a `type = "mesh"` flag. Updates available model list without restart. Nice-to-have but not blocking since static config works.

6. **Landing page** -- Static site at arbstr.com. Write last when you know what shipped. Key sections: what is arbstr (NiceHash for AI inference), anti-token manifesto, getting started (`docker compose up`), architecture diagram.

### Defer to post-v2.0:

- **Buyer Cashu deposit endpoint**: Vault-side feature. For v2.0 testing, seed balances directly in vault DB or via vault admin API.
- **L402 anonymous access**: Future milestone.
- **Seller accounts / withdrawals**: Explicitly future scope.
- **Cross-node federation**: Explicitly future scope.
- **Dynamic provider pricing**: Marketplace-scale complexity.

## Complexity Assessment

| Feature | Effort | Risk | Notes |
|---------|--------|------|-------|
| Vault billing in handlers | 2-3 days | Medium | Handler integration touches hot path. Must handle streaming + non-streaming, timeout edge cases, agent token extraction. Existing vault client code is solid. |
| mesh-llm static config | 0.5 day | Low | Config and docs only. Already works -- just needs documentation and a tested example. |
| mesh-llm model sync | 1-2 days | Low | HTTP polling + provider model list refresh. Watch for race conditions with in-flight requests during model list update. |
| Docker Compose hardening | 2-3 days | Medium | Multi-stage Dockerfiles, health check timing, volume permissions, .env template. Integration testing across 4 services is the time sink. |
| Vault integration tests | 1-2 days | Low | Mock vault server in test harness. Test reserve/settle/release/failure paths. Pending settlement persistence verification. |
| Landing page | 2-3 days | Low | Content creation, not engineering. Static HTML/CSS. Design and copy are the bottleneck. |
| Buyer Cashu deposit | 1-2 days | Medium | Vault-side work. Cashu token parsing, mint redemption, balance credit. Error handling for invalid/spent tokens. |

## Sources

- [mesh-llm documentation](https://docs.anarchai.org/) -- OpenAI-compatible API on localhost:9337, Nostr auto-discovery with `--auto` flag, 15+ model catalog
- [mesh-llm GitHub](https://github.com/michaelneale/mesh-llm) -- Reference implementation, GGUF model support, distributed inference across nodes
- [NiceHash hashpower marketplace](https://www.nicehash.com/marketplace) -- Buyer/seller compute marketplace model, real-time pricing, auto-switching algorithms, BTC settlement
- [NiceHash review (Coin Bureau)](https://coinbureau.com/review/nicehash) -- 2.5M+ users, 250K daily active miners, buyer/seller role switching
- [L402 protocol for AI payments](https://bingx.com/en/learn/article/what-is-l402-payments-for-ai-agents-on-lightning-network-how-does-it-work) -- Lightning-native HTTP 402 payment protocol, per-request micropayments
- [Cashu ecash protocol](https://cashu.space/) -- Bearer ecash tokens, self-hosted mints, NUT specifications
- [Cashu Dev Kit](https://cashudevkit.org/introduction/) -- Developer libraries for wallet and mint integration
- [AI compute marketplaces 2026](https://www.artificialintelligence-news.com/news/top-5-ai-compute-marketplaces-reshaping-the-landscape-in-2026/) -- Competitive landscape: Render, Node AI, GPU aggregators
- [Dev tool landing pages study](https://evilmartians.com/chronicles/we-studied-100-devtool-landing-pages-here-is-what-actually-works-in-2025) -- 100+ dev tool landing pages analyzed for what works
- [Top 5 AI-Crypto projects 2026](https://medium.com/@XT_com/top-five-ai-crypto-projects-leading-decentralized-ai-in-2026-1fd3b2d3ec91) -- Bittensor, Render, SingularityNET competitive context
- arbstr `vault.rs` (local) -- Existing VaultClient with reserve/settle/release, retry, pending persistence, reconciliation loop
- arbstr-node `docker-compose.yml` (local) -- Existing scaffold with lnd + mint + vault + core services
- arbstr PROJECT.md (local) -- Requirements, out of scope, future features, key decisions
