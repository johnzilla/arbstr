# Project Research Summary

**Project:** arbstr - Reliability and observability milestone
**Domain:** LLM routing proxy with Bitcoin/Cashu payments
**Researched:** 2026-02-02
**Confidence:** MEDIUM

## Executive Summary

arbstr is an intelligent LLM routing proxy that optimizes cost across Routstr marketplace providers. The next milestone adds reliability (retry/fallback) and observability (request logging, cost tracking) to the existing MVP. Research shows this domain has converged on standard patterns: every production LLM proxy implements provider fallback, retry with backoff, request logging to persistent storage, and token/cost tracking. arbstr's unique context (Bitcoin-native payments via Cashu tokens, single-user local deployment) means some enterprise features (web dashboards, multi-tenancy, API key management) are deliberately excluded anti-features.

The recommended approach builds on arbstr's existing solid foundation: the Rust/Tokio/axum/sqlx stack is already complete and requires minimal changes (just adding two sqlx feature flags). The architecture should extend, not replace, existing components - extracting retry logic into a new RequestExecutor component while keeping handlers focused on HTTP concerns, adding async request logging via SQLite with fire-and-forget pattern, and wrapping streaming responses to detect errors and extract token counts. The critical path is: fix cost calculation first (currently broken - uses only output_rate), then add observability (logging with correlation IDs), then layer reliability on top.

The key risk is Cashu token double-spend during retries: if a request fails after the token is submitted to the provider, naive retry sends the same token again, causing financial loss. Prevention requires classifying failures as "safe to retry" (connection refused before request sent) versus "unsafe to retry" (timeout after token submission), and never auto-retrying the latter without explicit user confirmation. Other critical pitfalls include silent mid-stream failures in SSE responses (must inject error events) and SQLite blocking the async runtime (must use fire-and-forget logging with dedicated writer task).

## Key Findings

### Recommended Stack

arbstr's existing stack is well-chosen and requires only minimal enhancement. The milestone needs no new dependencies - everything required for reliability and observability is already in Cargo.toml. The only change needed is adding two feature flags to sqlx: "migrate" (for embedded migration support) and "chrono" (for native DateTime mapping). This deliberate minimalism reflects good initial design.

**Core technologies:**
- **sqlx 0.8** (SQLite): Async-native database with compile-time query checking - already declared but unused, perfect for request logging with WAL mode for concurrent read/write
- **futures 0.3**: Stream combinators for SSE parsing and error detection - already available, no new dependency needed for streaming error handling
- **tracing + tracing-subscriber**: Structured logging with span-based correlation - already configured, just needs request ID spans for observability
- **Custom retry logic**: Provider fallback requires switching providers on failure, which doesn't fit tower-retry's same-service retry model - 20-30 lines of custom code beats forcing a library abstraction

**Key decisions:**
- NO tiktoken dependency - extract token counts from provider responses (authoritative) not local tokenization (adds 10MB+ model files)
- NO circuit breaker crate - simple provider health tracking in DashMap/RwLock sufficient for 2-5 providers on single-user proxy
- NO metrics crate - single-user local proxy doesn't need Prometheus exporters, SQLite queries serve the use case better

### Expected Features

Research analyzed LiteLLM, Portkey, Helicone, and BricksLLM to identify convergence patterns.

**Must have (table stakes):**
- Provider fallback on failure - every proxy does this; returning error on first provider failure provides no value over direct API calls
- Retry with exponential backoff - transient errors (429, 500, 502, 503) are common; 2-3 max retries with jitter is standard
- Request logging to SQLite - every observability product starts here; captures timestamp, model, provider, tokens, cost, latency, success
- Token count extraction - usage data appears in OpenAI-compatible responses; streaming requests need SSE parsing for final chunk usage field
- Cost calculation fixing - current code uses only output_rate; must use full formula: (input_tokens * input_rate + output_tokens * output_rate) / 1000 + base_fee
- Latency measurement - wall-clock time per request, critical for validating proxy overhead
- Stream error handling - detect mid-stream disconnects and signal cleanly to client (no transparent retry for streams - partial content already sent)

**Should have (differentiators):**
- Cost tracking in satoshis with query endpoints - arbstr's unique value, enables "how much did I spend on code generation this week" queries
- Response metadata headers - X-Arbstr-Cost-Sats, X-Arbstr-Latency-Ms, X-Arbstr-Retries on every response (Portkey/Helicone pattern)
- Per-model and per-policy cost breakdown - simple SQL GROUP BY on logged data, high value for cost optimization
- Enhanced health endpoint - /health should reflect provider reachability, not static "ok"

**Defer (v2+):**
- Learned token ratios per policy - needs logged data to learn from, add after data collection period
- Circuit breaker - needs operational data to tune thresholds, add after observing real failure patterns
- Per-provider timeout configuration - global 120s works initially, refine based on observed latency profiles

**Anti-features (deliberately NOT building):**
- Web dashboard UI - single-user proxy doesn't need frontend complexity, provide JSON query endpoints instead
- API key management / client authentication - single user on local network has no abuse vector
- Prompt caching / semantic cache - high complexity, single-user usage rarely produces exact duplicates
- Guardrails / content filtering - personal tool doesn't need PII detection between user and their own proxy
- Automatic model fallback - dangerous for cost optimization; silently routing gpt-4o to gpt-4o-mini changes quality

### Architecture Approach

The existing architecture is stateless and simple: Request → Handler → Router::select() → Forward → Response. The milestone extends this by extracting provider communication into separate components while preserving the handler's focus on HTTP concerns. Key pattern is "extract then enhance" - refactor provider calling into RequestExecutor as pure refactor first, then add retry logic to the focused component.

**Major components:**
1. **RequestExecutor** (new: src/proxy/executor.rs) - Encapsulates try/retry/fallback logic; receives ordered provider list from router, attempts in sequence, tracks metadata; keeps retry logic testable without spinning up axum server
2. **StreamWrapper** (new: src/proxy/stream.rs) - Wraps SSE byte streams to parse chunks, detect [DONE] sentinel, extract token usage from final chunk, inject error events on mid-stream failures; uses oneshot channel to send metadata back after stream completes
3. **Storage** (new: src/storage/mod.rs) - SQLite with sqlx pool; request logging via fire-and-forget tokio::spawn pattern (never block response path); migrations embedded via sqlx::migrate!; WAL mode for concurrent read/write
4. **HealthTracker** (new: src/router/health.rs) - In-memory provider health state (DashMap or RwLock<HashMap>); track consecutive failures and last-failure timestamp; deprioritize unhealthy providers during selection
5. **CostCalculator** (new: src/router/cost.rs) - Pure function for cost calculation; fixes current bug (only uses output_rate); implements full formula with input_rate + output_rate + base_fee

**Critical patterns:**
- Fire-and-forget logging: tokio::spawn(storage.log_request()) after response - never await database write in request path
- Ordered provider list for retry: Router returns [cheapest, next_cheapest, ...], executor tries in sequence, excludes failures
- Oneshot channel for stream metadata: streaming responses send token counts back via channel after final chunk parsed
- No retry for streaming: once streaming starts, partial content sent to client; retry would duplicate content

### Critical Pitfalls

Based on analysis of the codebase and domain patterns:

1. **Cashu token double-spend on retry** - If provider request fails after token submission, retrying with same token causes financial loss (token already redeemed). Prevention: classify failures as "safe to retry" (connection refused before send) vs "unsafe" (timeout/5xx after send); never auto-retry unsafe failures; use fresh token for retries or mark original as "possibly spent" for reconciliation.

2. **Silent mid-stream SSE failures** - Current code pipes bytes directly (handlers.rs:87-104); if provider disconnects mid-stream, client gets truncated response with no error indication. Prevention: parse SSE events during streaming, track [DONE] sentinel, inject `data: {"error": ...}` event on upstream failure before closing client stream.

3. **SQLite blocking Tokio runtime** - Synchronous SQLite writes hold file-level lock; if logging blocks in request handler's await chain, all concurrent requests hang. Prevention: fire-and-forget pattern (tokio::spawn for writes), WAL mode (PRAGMA journal_mode=WAL), bounded channel with write batching, drop log entries if channel full rather than blocking requests.

4. **Broken cost calculation** - Current code uses only output_rate (selector.rs:170-172); ignores input_rate and base_fee; breaks cost tracking and suboptimal provider selection for high-input-token workloads. Prevention: implement full formula BEFORE adding logging (pollutes historical data); estimate input tokens from request, output from max_tokens or learned ratios.

5. **Retry without backoff creates thundering herd** - When provider has transient overload, immediate retries from concurrent users amplify the problem. Prevention: exponential backoff with jitter (500ms, 1s, 2s with ±25% random), circuit breaker (stop trying provider after N failures for cooldown period), respect Retry-After headers.

## Implications for Roadmap

Research reveals strong dependency ordering: observability must precede reliability because you need to see failures to handle them well. With logging in place, you can tune retry policies empirically rather than guessing.

### Phase 1: Foundation - Cost Calculation and Correlation
**Rationale:** Fix cost calculation before any logging starts - if cost numbers are wrong from day one, historical data is polluted and learned token ratios will be incorrect. Add request correlation IDs before implementing other features because all subsequent debugging depends on tracing requests through logs.
**Delivers:** Correct cost calculation with full formula (input + output + base), request ID generation and span-based tracing
**Addresses:** Pitfall 4 (broken cost calculation), Pitfall 12 (no correlation IDs)
**Implementation:** ~60 LOC - cost formula in src/router/cost.rs, UUID generation in handlers, tracing span wrapper

### Phase 2: Observability - Request Logging and Cost Tracking
**Rationale:** Logging is the foundation for everything downstream - cost tracking, health monitoring, learned patterns all need logged data. Must implement correctly from start with fire-and-forget pattern to avoid Pitfall 3.
**Delivers:** SQLite storage with migrations, async request logger, cost query endpoints (/stats, /stats/models), response metadata headers
**Addresses:** Table stakes (request logging, latency measurement, cost tracking), Pitfall 3 (SQLite blocking), Pitfall 6 (logging bodies)
**Uses:** sqlx with migrate+chrono features, fire-and-forget tokio::spawn pattern
**Implementation:** ~300 LOC - storage module, migrations, handler integration, query endpoints

### Phase 3: Streaming Observability - SSE Parsing
**Rationale:** Streaming error handling and token counting are coupled - both require parsing SSE chunks. Implement before retry logic because streaming requests cannot be retried (content already sent), so the error detection path is simpler.
**Delivers:** StreamWrapper that parses SSE events, extracts usage field from final chunk, injects error events on failures, validates [DONE] sentinel
**Addresses:** Pitfall 2 (silent stream failures), Pitfall 7 (streaming token counting), table stakes (stream error handling)
**Uses:** futures::StreamExt, tokio::sync::oneshot for metadata channel
**Implementation:** ~150 LOC - stream wrapper module, SSE parser, error injection

### Phase 4: Reliability - Retry and Fallback
**Rationale:** Comes last because it's most complex and benefits from all observability built in prior phases. Need logging to validate retry logic works correctly, need stream error handling to know when NOT to retry.
**Delivers:** RequestExecutor with retry loop, HealthTracker for provider state, Router::select_with_fallbacks, exponential backoff with jitter
**Addresses:** Table stakes (retry with backoff, provider fallback), Pitfall 1 (Cashu double-spend), Pitfall 5 (thundering herd), Pitfall 8 (model fallback), Pitfall 9 (timeout configuration)
**Implements:** RequestExecutor component, HealthTracker component
**Implementation:** ~250 LOC - executor module, health tracker, retry logic, handler refactor

### Phase 5: Polish - Enhanced Endpoints
**Rationale:** Nice-to-haves that leverage the infrastructure built in prior phases. Low risk, incremental value.
**Delivers:** /health with provider status, cost breakdown endpoints, config for retry policy
**Addresses:** Pitfall 11 (useless health check), Pitfall 10 (error format consistency)
**Implementation:** ~100 LOC - enhanced handlers, config schema updates

### Phase Ordering Rationale

- **Cost calculation first** because it's a prerequisite for accurate logging - fixing it after logging starts pollutes historical data
- **Logging before reliability** because observability informs tuning - you need to see failure patterns to design good retry policies
- **Streaming before retry** because streaming cannot be retried (fundamental constraint) so its error path is simpler and validates patterns before tackling complex retry logic
- **Polish last** because it's low-risk incremental improvements that leverage prior work

This order also minimizes rework: each phase builds cleanly on prior phases without needing to go back and change earlier implementations.

### Research Flags

**Phases likely needing deeper research during planning:**
- **Phase 4 (Reliability):** Cashu token payment semantics need verification - specifically what happens when same token submitted twice, whether Routstr providers expose a reconciliation endpoint, and whether there's a standard way to mark tokens as "possibly spent". Training data doesn't have specific Routstr provider behavior.
- **Phase 3 (Streaming):** Need to verify whether Routstr providers include `usage` field in final SSE chunk (OpenAI pattern) or if arbstr needs to estimate from chunk counts. Affects accuracy of streaming cost tracking.

**Phases with standard patterns (skip research-phase):**
- **Phase 1 (Cost Calculation):** Pure arithmetic, well-specified in provider config schema
- **Phase 2 (Request Logging):** Standard sqlx + SQLite patterns, well-documented
- **Phase 5 (Polish):** Straightforward handler additions

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Existing dependencies are correct; only sqlx feature flags needed; custom retry approach is sound based on Tower's documented limitations |
| Features | HIGH | Convergence across LiteLLM/Portkey/Helicone/BricksLLM is clear; table stakes vs differentiators well-supported by competitive analysis |
| Architecture | MEDIUM | Patterns (fire-and-forget logging, oneshot for streams) are well-established in Rust async; but exact SSE parsing behavior with different clients not verified |
| Pitfalls | HIGH | Pitfalls 1-4 directly observable in codebase or inherent to tech (Cashu, SQLite concurrency); 5-12 are standard domain patterns |

**Overall confidence:** MEDIUM

Training data knowledge is comprehensive for the Rust ecosystem and LLM proxy patterns (January 2025 cutoff), but two areas lack verification:
1. Specific Routstr provider behavior (SSE usage field inclusion, Cashu token reconciliation endpoints)
2. Current crate versions (sqlx 0.8 was current at training cutoff; minor releases may have occurred)

### Gaps to Address

- **Cashu token double-spend semantics:** Need to research Routstr-specific behavior - do providers expose a way to check token status? Can tokens be marked as reserved before submission? What happens on duplicate submission? This directly affects retry safety classification.

- **Routstr SSE streaming format:** Verify whether Routstr providers include OpenAI-compatible `usage` field in final SSE chunk, or if arbstr needs to estimate from chunk counts/character counts. Affects streaming cost tracking accuracy.

- **sqlx version verification:** Confirm sqlx 0.8 is still current; check for breaking changes if 0.9 exists. Low risk - cargo build will surface issues.

- **OpenAI client library error handling:** Test error events in SSE streams against Python OpenAI SDK, Cursor, and Claude Code to verify they parse injected error events correctly. Affects streaming error detection UX.

## Sources

### Primary (HIGH confidence)
- arbstr codebase analysis (src/proxy/handlers.rs, src/router/selector.rs, src/config.rs, src/error.rs, Cargo.toml) - direct observation of current implementation and gaps
- CLAUDE.md project documentation - architecture decisions, milestone roadmap, schema design
- sqlx 0.8 documentation and Tower 0.4 API semantics - training data knowledge as of January 2025

### Secondary (MEDIUM confidence)
- LiteLLM, Portkey, Helicone, BricksLLM feature comparison - training data knowledge of these products' feature sets and positioning as of early 2025; web verification unavailable
- Rust async patterns (Tokio spawn, oneshot channels, fire-and-forget) - well-established community practices from training data
- SQLite WAL mode and concurrency behavior - documented SQLite features

### Tertiary (LOW confidence, needs validation)
- Routstr provider SSE format specifics - assumed OpenAI-compatible but not verified
- Cashu token redemption semantics in context of retries - general Cashu knowledge but not Routstr-specific behavior
- Exact behavior of OpenAI client SDKs parsing custom SSE error events - pattern is sound but client-specific handling not verified

---
*Research completed: 2026-02-02*
*Ready for roadmap: yes*
