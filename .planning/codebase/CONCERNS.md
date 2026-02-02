# Codebase Concerns

**Analysis Date:** 2026-02-02

## Tech Debt

**Missing Database Logging Implementation:**
- Issue: Database schema and logging infrastructure defined in config but not implemented. Cost tracking is essential for arbstr's core function (cost optimization), yet requests are never persisted to SQLite.
- Files: `src/proxy/handlers.rs:120`, `src/config.rs:31-49` (DatabaseConfig struct), `src/main.rs:148-150` (mock database setup)
- Impact: Requests are lost after response. Cannot track costs over time, cannot learn token ratios, cannot validate cost savings claims. Core feature of roadmap (Phase 2: Intelligence) is blocked.
- Fix approach: Implement `src/storage/` module with request logging. Create migration runner to initialize SQLite schema. Add async database write in `chat_completions` handler after cost calculation.

**Incomplete Routing Strategies:**
- Issue: Two routing strategies declared but not implemented - `lowest_latency` and `round_robin` both fall back to selecting first provider.
- Files: `src/router/selector.rs:92-93`
- Impact: Policies specifying these strategies will not work as intended. Users requesting latency-based routing will get deterministic (first provider) selection instead.
- Fix approach: Implement latency tracking in request log. Add round-robin state to `Router` struct with mutex-protected counter. Add corresponding strategies to `select()` method.

**Incomplete Cost Calculation:**
- Issue: Cost is calculated only on output rate (line 171 in selector.rs). Input rate is recorded in config but never used. Base fees are stored but ignored in selection logic.
- Files: `src/router/selector.rs:171`, cost calculation appears nowhere in request handling
- Impact: Provider selection is incorrect. A provider with high input rate and low output rate looks cheaper than it actually is. Base fees never factored into decision.
- Fix approach: Change `select_cheapest()` to estimate full cost: `(input_tokens * input_rate + output_tokens * output_rate + base_fee) / 1000`. Requires token count from user request or estimation from prompt length.

**Unimplemented Feature: Heuristic Policy Matching:**
- Issue: Keyword matching for policies (lines 114-125 in selector.rs) is simplistic substring matching. Cannot handle variations ("code", "coding", "encode"), case sensitivity on keywords stored in config. No weight system for multiple matching keywords.
- Files: `src/router/selector.rs:114-125`
- Impact: Policy heuristics unreliable. A request with "encoding" won't match policy keyword "code". Order of policies matters (first match wins), not quality of match.
- Fix approach: Use fuzzy matching library (strsim crate). Normalize both prompt and keywords to lowercase. Return best match by confidence score, not first match.

**API Key Exposure Risk:**
- Issue: API keys are stored in plaintext in config files and printed in debug output. No validation that api_key is required when provider needs it.
- Files: `src/config.rs:59`, `src/main.rs:120-133` (prints provider config including URLs)
- Impact: Keys visible in config.toml, process listings, logs. User error if they commit config.toml with real keys. No warning system.
- Fix approach: Load API keys from environment variables, not config file. Add validation to ensure required auth fields are set. Never log api_key values (use masking in debug output).

## Known Bugs

**Streaming Response Loss on Provider Failure:**
- Symptoms: If provider disconnects mid-stream, client receives incomplete response with no error indication.
- Files: `src/proxy/handlers.rs:89-104`
- Trigger: Provider closes connection during SSE stream; network latency spike causes socket timeout
- Current behavior: Stream ends silently; client receives partial chunks
- Workaround: Retry from client side (OpenAI SDK will retry non-200 responses but not streaming errors)
- Fix approach: Wrap byte_stream in error handler. Send error event to client before closing stream if downstream fails.

**Configuration Validation Insufficient:**
- Symptoms: Empty provider list is warned but allowed, then all requests fail with "no providers available"
- Files: `src/config.rs:154-169`
- Trigger: User provides empty config or all providers have no matching models
- Current behavior: Warning logged at startup but proxy still starts and accepts requests
- Workaround: None; have to restart with valid config
- Fix approach: Fail validation if no providers configured OR no policies match available providers.

**Request Body Size Unbounded:**
- Symptoms: No limit on incoming request size. Large file uploads in messages will consume unbounded memory.
- Files: `src/proxy/server.rs`, axum router setup - no body size limit configured
- Trigger: POST request with multi-megabyte message content
- Current behavior: Request buffered entirely in memory before forwarding
- Workaround: Use reverse proxy (nginx) in front to limit request size
- Fix approach: Add `DefaultBodyLimit` layer to axum router with reasonable max (e.g., 10MB for chat).

## Security Considerations

**Unvalidated Passthrough of Upstream Responses:**
- Risk: Non-streaming responses are parsed from JSON and re-serialized, but no validation of schema. If upstream provider returns malicious JSON structure, it's passed directly to client.
- Files: `src/proxy/handlers.rs:107-118`
- Current mitigation: serde_json validation (basic type checking) before forwarding
- Recommendations: Validate response structure matches OpenAI schema. Reject responses with unknown fields (security). Add response size limits.

**No Input Validation on Model Name:**
- Risk: Model parameter taken directly from request without validation against config. No injection protection.
- Files: `src/proxy/handlers.rs:20-48`, `src/router/selector.rs:58-100`
- Current mitigation: None; model name used as string in logs and passed to upstream
- Recommendations: Validate model against allowed_models in policy or all configured provider models. Reject unknown models before selecting provider.

**Prompt Content Used for Heuristic Matching:**
- Risk: User prompt (potentially sensitive) is read for policy heuristics. Sensitive information in prompts could be logged or matched against keywords unexpectedly.
- Files: `src/proxy/handlers.rs:29`, `src/router/selector.rs:113-125`
- Current mitigation: Prompt matching is optional; header-based policy takes precedence
- Recommendations: Add config flag to disable prompt-based heuristics. Never log matched prompt content in policy debug logs. Consider privacy implications of keyword matching.

**No Rate Limiting:**
- Risk: Proxy accepts unlimited requests per second. No protection against request flooding from single client or coordinated attacks.
- Files: No rate limiting middleware in `src/proxy/server.rs`
- Current mitigation: None
- Recommendations: Add tower rate limit middleware. Implement per-IP or per-API-key limits. Consider fair sharing across multiple clients.

## Performance Bottlenecks

**Provider List Filtering on Every Request:**
- Problem: `chat_completions` handler filters providers list on every request, searching for model support, applying policy constraints
- Files: `src/router/selector.rs:68-83`
- Cause: Vec iteration with filters instead of pre-indexed lookup
- Impact: O(n) lookup for m candidates across n providers, repeated per request. Scales poorly with many providers.
- Improvement path: Build provider index during config load (HashMap by model -> [provider_ids]). Memoize policy matches. Cache constraint-filtered provider lists if policies don't change at runtime.

**Keyword Matching Case Sensitivity:**
- Problem: `prompt.to_lowercase()` on every request for every policy rule keyword comparison
- Files: `src/router/selector.rs:114-125`
- Cause: Keywords stored in mixed case in config, lowercased at match time
- Impact: Extra string allocations and lowercasing on every request with multiple policies
- Improvement path: Lowercase keywords once during config parsing. Store normalized keywords in policy struct.

**Hardcoded HTTP Client Timeouts:**
- Problem: Timeout values (120s total, 10s connect) are fixed in code, not configurable
- Files: `src/proxy/server.rs:51-52`
- Cause: No timeout config in `ServerConfig` or `ProviderConfig`
- Impact: Cannot optimize for slow providers or fast-fail on unresponsive ones without code change
- Improvement path: Add timeout fields to `ProviderConfig`. Use per-provider timeouts in handler. Add global defaults in ServerConfig.

## Fragile Areas

**OpenAI Compatibility Layer:**
- Files: `src/proxy/types.rs` (request/response types), `src/proxy/handlers.rs` (endpoint implementations)
- Why fragile: Custom OpenAI types don't validate against actual OpenAI API. Response types manually constructed instead of using official types. Any OpenAI API change (new field, removed field) breaks compatibility without test signal.
- Safe modification: Add integration tests against real OpenAI API and Routstr providers. Validate against OpenAI JSON schema. Consider using `openai-api-rs` crate for shared types.
- Test coverage: Zero integration tests with real APIs. Only unit tests with mock data in `src/router/selector.rs`.

**Policy Constraint Logic:**
- Files: `src/router/selector.rs:131-167` (apply_policy_constraints)
- Why fragile: Multiple constraint filtering passes. If policy rule has both `allowed_models` and `max_sats_per_1k_output`, order matters. No clear error message when all providers filtered out.
- Safe modification: Add comprehensive policy constraint tests with edge cases (no providers left, overlapping constraints). Return detailed error why provider was rejected (not in allowed_models vs exceeds cost).
- Test coverage: Only one policy test case (`test_policy_keyword_matching`), no edge cases.

**Async Error Handling in Streaming:**
- Files: `src/proxy/handlers.rs:87-104` (streaming response)
- Why fragile: `bytes_stream()` errors are silently wrapped as `io::Error`. If provider fails mid-stream, client receives incomplete response. Error callback in map closure has no way to signal back to handler.
- Safe modification: Use `try_stream` or error handling middleware. Consider tokio-tungstenite for proper WebSocket-style error signaling on streams.
- Test coverage: No tests for streaming error scenarios.

## Scaling Limits

**Single Process Memory:**
- Current capacity: Depends on Tokio task concurrency (default 512) and request size. Each streaming request holds one TCP connection and one upstream socket open.
- Limit: With 1000 concurrent requests at 10MB each request buffer, ~10GB memory. Process will OOM.
- Scaling path: Add request body size limits. Implement connection pooling to reuse upstream sockets. Add horizontal scaling with load balancer (stateless proxy, logging to shared database).

**Provider Configuration Refresh:**
- Current capacity: Static config loaded at startup
- Limit: Cannot add/remove providers without restart. Cannot change provider rates without downtime.
- Scaling path: Implement hot config reload (watch config file, rebuild router on change). Add provider management API. Store provider list in database with versioning.

**SQLite Database Bottleneck:**
- Current capacity: SQLite handles ~100 concurrent writes reasonably
- Limit: Once cost logging implemented, each request triggers database write. 1000 req/sec = 1000 writes/sec, SQLite will contend
- Scaling path: Use write-ahead logging (WAL) in SQLite for concurrent readers. Batch inserts (accumulate requests in memory, flush every second). Migrate to PostgreSQL for horizontal scaling.

## Dependencies at Risk

**reqwest Upgrade Potential:**
- Risk: Reqwest 0.12 may have breaking changes in future versions. Custom timeout configuration might break.
- Current use: HTTP client for upstream provider calls
- Impact: Version pinned in Cargo.toml; if reqwest 1.x breaks API, requires code updates
- Migration plan: Monitor reqwest releases. Consider using Tower's http-client abstraction to reduce coupling.

**sqlx Runtime Startup:**
- Risk: sqlx is enabled with `runtime-tokio` feature but database is never initialized (see tech debt). When database logging is implemented, startup will fail if SQLite driver not available.
- Current use: Imported but not used
- Impact: Dependency bloat; misleading that database is ready
- Migration plan: Remove sqlx from dependencies until database module is implemented. Add as dependency only when storage module added.

## Missing Critical Features

**Request Logging:**
- Problem: No request history. Cannot see which provider was selected for each request, actual costs incurred, token usage patterns.
- Blocks: Cost tracking, learning token ratios, arbitrage analysis, debugging provider issues
- Severity: High - this is core MVP Phase 2 feature

**Cost Tracking Dashboard:**
- Problem: No visibility into cost savings. Users don't know if they're actually saving money using arbstr.
- Blocks: Product validation, user confidence in routing decisions
- Severity: High - MVP requirement

**Token Count Estimation:**
- Problem: Cost calculation stub doesn't use actual token counts. Estimates based on provider advertised rates only.
- Blocks: Accurate cost prediction, intelligent routing decisions, temporal arbitrage
- Severity: Medium - affects correctness of cost optimization

## Test Coverage Gaps

**Untested: Provider Selection Edge Cases:**
- What's not tested: Multiple providers with same output rate (tie-breaking behavior undefined). Empty provider list. Providers with overlapping model lists.
- Files: `src/router/selector.rs`
- Risk: Tie-breaking is undefined (min_by_key takes first). Could select unexpectedly different provider on reconfig.
- Priority: Medium

**Untested: Policy Constraint Combinations:**
- What's not tested: Policy with allowed_models that no provider supports. Policy with max_sats that no provider meets. Multiple policies matching same prompt.
- Files: `src/router/selector.rs:131-167`
- Risk: Ambiguous behavior, unclear error messages when all providers filtered out.
- Priority: High

**Untested: HTTP Streaming Failures:**
- What's not tested: Provider drops connection mid-stream. Timeout during streaming response. Malformed SSE events from provider.
- Files: `src/proxy/handlers.rs:87-104`
- Risk: Silent failures, client receives incomplete response without error.
- Priority: High

**Untested: Configuration Validation:**
- What's not tested: Circular policy references. Invalid URL formats. Models with special characters. Missing required fields.
- Files: `src/config.rs:154-169`
- Risk: Invalid configs accepted, fail at runtime instead of startup.
- Priority: Medium

**Untested: Error Path Serialization:**
- What's not tested: Does error response match OpenAI error format exactly? What if upstream returns non-JSON?
- Files: `src/error.rs:34-57`, `src/proxy/handlers.rs:74-85`
- Risk: Client errors if response format doesn't match expectations.
- Priority: Medium

**Untested: Keyword Heuristics Matching:**
- What's not tested: Case sensitivity (keyword "Code" vs prompt "code"). Substring overlap (keyword "code" matches "decode"). Empty keywords. Very long prompts.
- Files: `src/router/selector.rs:114-125`
- Risk: Policies match unexpectedly or fail to match intended prompts.
- Priority: Medium

---

*Concerns audit: 2026-02-02*
