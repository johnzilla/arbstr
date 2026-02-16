# Project Research Summary

**Project:** arbstr v1.2 - Streaming Observability
**Domain:** SSE stream interception and token extraction for OpenAI-compatible proxy
**Researched:** 2026-02-15
**Confidence:** HIGH

## Executive Summary

arbstr v1.2 aims to close a critical observability gap: streaming responses currently bypass token counting and cost tracking entirely, logging NULL values for input/output tokens and cost. The research reveals that this is a solvable problem using existing dependencies—no new crates required. The OpenAI API provides `stream_options: {"include_usage": true}` to request a final SSE chunk containing authoritative token counts before `data: [DONE]`. By injecting this parameter into upstream requests, parsing the final chunk, and updating the database after stream completion, arbstr can achieve complete observability parity between streaming and non-streaming requests.

The technical approach leverages tools already in the dependency tree: `reqwest::bytes_stream()` for chunk delivery, `futures::StreamExt::map()` for pass-through interception, `Arc<Mutex<Option<Usage>>>` for capturing extracted values (mirroring the existing `retry.rs` pattern), and `tokio::sync::oneshot` for post-stream database updates. The critical implementation requirement is SSE line buffering across TCP chunk boundaries—the current `text.lines()` approach silently drops usage data when chunks split mid-JSON. This is a well-understood networking pitfall with a standard solution (maintain a String buffer, process complete lines, retain partial trailing lines).

The main risk is provider compatibility. While OpenAI, vLLM, and recent Ollama versions support `stream_options`, Routstr marketplace providers have unknown compatibility. The recommended mitigation: inject `stream_options` by default but gracefully handle streams that complete without usage data by logging NULL values (current behavior). This safe degradation ensures no regression—unsupported providers continue working exactly as they do today, while supported providers gain observability.

## Key Findings

### Recommended Stack

**No new dependencies needed.** The existing stack already contains everything required for streaming token extraction. This milestone uses zero new crates—a significant validation that the v1/v1.1 stack choices were sound.

**Core technologies (already in Cargo.toml):**
- **futures 0.3.31**: StreamExt for `.map()` stream transformation (already used in `handle_streaming_response`)
- **tokio (full features)**: oneshot channel for post-stream notification, spawn for fire-and-forget UPDATE
- **serde_json 1.x**: Parse SSE data payloads (`data: {"usage":...}`) to extract token values
- **sqlx 0.8 (sqlite)**: UPDATE query for post-stream token/cost amendment, correlation_id already indexed
- **reqwest 0.12 (stream feature)**: `bytes_stream()` already delivers SSE chunks, no changes needed
- **bytes 1.11.0 (transitive)**: Already available via axum/reqwest, no direct import needed
- **pin-project-lite 0.2.16 (transitive)**: Available if needed for custom Stream types (likely unnecessary)

**Why NOT eventsource-stream**: OpenAI SSE format is trivial (`data: {json}\n\n`). Hand-rolled parsing via `strip_prefix("data: ")` already exists in the codebase and works. A library would add dependency weight for zero practical benefit—arbstr only needs the final usage chunk, not full SSE spec compliance (event types, IDs, retry semantics).

**Why NOT async-stream**: The existing `bytes_stream().map()` pattern is sufficient and already used. Introducing `stream!` macro would create two competing stream construction patterns for no gain.

**Why NOT client-side tokenizer (tiktoken-rs)**: Heavy dependency (~20MB vocab files), must match exact model tokenizer, inaccurate for structured output. Provider-reported usage from `stream_options` is authoritative. If a provider doesn't send usage, log NULL—don't estimate.

For full stack analysis, see [STACK.md](./STACK.md).

### Expected Features

**Must have (table stakes):**
- **SSE usage extraction from final chunk**: OpenAI sends `usage: {prompt_tokens, completion_tokens}` in the final chunk when `stream_options: {"include_usage": true}` is set. Every observability-aware proxy (LiteLLM, OpenRouter, Azure API Management) captures this. Without it, streaming requests have permanent cost-tracking blind spots.
- **Post-stream database UPDATE**: Log entry written before stream starts (fire-and-forget INSERT with NULL tokens). After stream completes, UPDATE the row with extracted usage and calculated cost via `correlation_id`.
- **Inject `stream_options` into upstream request**: Automatically add `stream_options: {"include_usage": true}` when `stream: true` to request usage data regardless of what the client sent. This is the enabler—without it, providers may not send usage at all.
- **SSE line buffering across chunk boundaries**: TCP chunks don't align with SSE line boundaries. A `data: {"usage":...}` line can split mid-JSON across chunks. Current `text.lines()` per chunk silently drops split lines. Fix: maintain String buffer across chunks, process complete lines only.
- **Latency reflects full stream duration**: Currently logged at request dispatch (time-to-first-byte). For streaming, should reflect time-to-last-byte. Capture in post-stream UPDATE.

**Should have (differentiators):**
- **Time-to-first-token (TTFT) metric**: Record elapsed time from request dispatch to first non-empty `delta.content` chunk. Key latency metric for streaming UX and provider quality assessment. Add `ttft_ms` column.
- **Stream completion status tracking**: Distinguish normal completion (`data: [DONE]`), interruption (connection dropped), and error (provider error chunk). Add `stream_status` column. Fixes data quality issue where all streams currently log as success.
- **Trailing SSE cost event**: After upstream `[DONE]`, inject `data: {"arbstr_cost_sats": X, "arbstr_latency_ms": Y}` before proxy closes stream. Gives clients real-time cost visibility without separate API call. Clean extension that clients can ignore if they don't understand arbstr events.

**Defer to v2:**
- **Output token counting by delta accumulation**: Only needed if Routstr providers lack `stream_options` support. Character-count heuristic (4 chars/token) can approximate, but adds complexity. Verify provider behavior first.
- **Streaming early-failure retry**: High complexity. Current fail-fast behavior is correct for most cases. Could detect early failures (error in first chunk) and retry before client receives data, but adds significant logic for edge cases.

For full feature analysis including anti-features and complexity estimates, see [FEATURES.md](./FEATURES.md).

### Architecture Approach

The recommended architecture follows an **insert-then-update pattern** that preserves the current safety property (requests logged even if streams fail) while adding post-completion data amendment. No schema changes needed—existing columns are nullable and support UPDATE. The flow: (1) inject `stream_options` before forwarding, (2) intercept stream with `.map()` closure that buffers lines and extracts usage into `Arc<Mutex<Option<Usage>>>`, (3) spawn completion task that awaits oneshot signal, (4) on stream end, task reads captured usage and executes fire-and-forget UPDATE with cost calculation.

**Major components:**
1. **Stream interception module (`src/proxy/stream.rs`, NEW)**: SseLineBuffer (handles chunk boundary splits), StreamUsage struct, parse_sse_usage (extracts from JSON), build_intercepted_stream (returns Body and shared usage handle), CompletionSignal (drop-based oneshot trigger for finalization)
2. **Request mutation (handlers.rs)**: inject_stream_options adds `{"include_usage": true}` to ChatCompletionRequest when stream=true, ensuring provider sends usage chunk regardless of client's request
3. **Post-stream database update (`storage/logging.rs`)**: update_streaming_usage executes `UPDATE requests SET input_tokens=?, output_tokens=?, cost_sats=?, latency_ms=? WHERE correlation_id=?`, spawned fire-and-forget after stream completes
4. **Type additions (`proxy/types.rs`)**: Add StreamOptions struct and stream_options field to ChatCompletionRequest (with skip_serializing_if = "Option::is_none" for backward compatibility)

**Data flow:** Client request → inject stream_options → provider → bytes_stream() → .map() closure (line buffer + usage extraction) → Body::from_stream → client receives identical bytes → stream ends → oneshot signals completion task → read shared usage → calculate cost → UPDATE database. The stream passes through unmodified—parsing is observation-only, zero latency added to content delivery.

For detailed implementation guidance, component diagrams, and build order, see [ARCHITECTURE.md](./ARCHITECTURE.md).

### Critical Pitfalls

1. **SSE data lines split across TCP chunk boundaries (CRITICAL)**: Current `text.lines()` per chunk assumes complete lines, but TCP delivers arbitrary byte boundaries. A `data: {"usage":...}` line can split mid-JSON. Result: `serde_json::from_str` fails silently, usage data lost. Fix: maintain String buffer across chunks, extract complete lines only. Test with mock that splits SSE events at every byte position.

2. **Database INSERT-then-UPDATE race (CRITICAL)**: Current fire-and-forget INSERT may not complete before stream ends (for small responses). UPDATE finds zero rows, data lost. Fix: either (A) await INSERT before starting stream, or (B) defer INSERT until stream completion (single write with complete data, but lose record if stream interrupted). Recommend (A) for safety—interrupted streams still logged.

3. **Providers that don't send usage data (CRITICAL)**: `stream_options` is OpenAI spec but adoption varies. Groq uses `x_groq.usage` (non-standard), Ollama support unclear, Routstr marketplace unknown. Injecting `stream_options` may break providers that reject unknown params. Fix: inject by default but handle streams ending without usage gracefully (log NULL, same as current behavior—safe degradation, no regression).

4. **Incorrect `data: [DONE]` handling (MODERATE)**: `[DONE]` is OpenAI convention, not SSE spec. If upstream drops connection, stream ends without `[DONE]`. Finalization logic must trigger on stream end (Poll::Ready(None)), not on `[DONE]` receipt. Use drop-based CompletionSignal or .chain() to detect stream exhaustion.

5. **Memory growth on long streams (MODERATE)**: Line buffer accumulates data. If not drained after processing complete lines, memory grows linearly with response size. Fix: drain buffer after extracting complete lines, retain only trailing partial line. Cap buffer at 64KB to prevent pathological cases.

For all 11 pitfalls with specific code locations, prevention strategies, and phase assignments, see [PITFALLS.md](./PITFALLS.md).

## Implications for Roadmap

Based on research, the v1.2 milestone decomposes into **3 focused phases** with clear boundaries.

### Phase 1: Foundation - Request Mutation and Database Infrastructure
**Rationale:** Enable the feature end-to-end without changing stream handling yet. Add `stream_options` field to request type, implement injection logic, add database UPDATE function. This is the foundation—everything else builds on these changes.

**Delivers:**
- `StreamOptions` type and `stream_options` field on `ChatCompletionRequest` (fully backward compatible)
- `inject_stream_options()` function that adds `{"include_usage": true}` when `stream: true`
- `update_streaming_usage()` in storage/logging.rs (UPDATE query by correlation_id)
- Integration into `send_to_provider` (call inject before forwarding)

**Addresses:**
- Must-have: "Inject stream_options into upstream request" (the critical enabler)
- Must-have: "Post-stream database UPDATE" (infrastructure, not yet called)
- Pitfall 10: ChatCompletionRequest doesn't forward unknown fields (fixes silent parameter dropping)

**Avoids:**
- No stream interception yet, so no tee architecture complexity
- No line buffering yet, so no memory management issues
- Can be tested independently with mock provider responses

**Research flag:** SKIP—standard Rust type additions and SQL UPDATE. Well-understood patterns already in codebase (fire-and-forget spawn_log_write).

### Phase 2: Core - SSE Stream Interception and Usage Extraction
**Rationale:** Implement the stream wrapper that intercepts bytes, buffers lines, and extracts usage. This is the highest-complexity phase but isolated to the new `stream.rs` module. Full unit test coverage before integration.

**Delivers:**
- `src/proxy/stream.rs` module (NEW)
  - `SseLineBuffer` with cross-chunk buffering
  - `StreamUsage` struct
  - `parse_sse_usage()` function (extracts from final chunk)
  - `build_intercepted_stream()` (returns Body + Arc<Mutex<Option<Usage>>>)
  - `CompletionSignal` (drop-based oneshot trigger)
- Unit tests for line buffering (partial chunks, multi-line chunks, edge cases)
- Unit tests for usage parsing (usage chunk, content chunk, [DONE], malformed JSON)

**Addresses:**
- Must-have: "SSE usage extraction from final chunk" (core parsing logic)
- Must-have: "SSE line buffering across chunk boundaries" (fixes silent data loss)
- Pitfall 1: Chunk boundary splits (the critical implementation requirement)
- Pitfall 7: SSE comment lines and keep-alive (handle gracefully)
- Pitfall 8: Memory growth on long streams (buffer drain logic)

**Avoids:**
- Pitfall 2: Tee architecture complexity (use existing .map() pattern, no channel overhead)
- Pitfall 6: Send + 'static constraints (clone only needed values into closure)
- Pitfall 9: Parsing overhead (substring check for "usage" before JSON parse)

**Research flag:** REVIEW—SSE parsing is well-documented but chunk boundary buffering has subtle edge cases. Consider referencing `eventsource-stream` source for buffering strategy (without adding dependency).

### Phase 3: Integration - Wire Stream Wrapper into Handlers
**Rationale:** Connect the stream wrapper to the request handler and trigger post-stream UPDATE. This is where all pieces come together. Existing integration tests must still pass (streaming works unmodified), plus new tests verify database gets updated.

**Delivers:**
- `handle_streaming_response` refactored to use `build_intercepted_stream()`
- `RequestOutcome` gains `shared_usage: Option<Arc<Mutex<Option<StreamUsage>>>>` field
- `chat_completions` streaming path spawns completion task with oneshot channel
- `spawn_stream_completion()` orchestrator function (awaits stream end, reads usage, calls UPDATE)
- Migration (if adding TTFT/stream_status columns for differentiators)
- Integration tests: stream with usage, stream without usage, client disconnect, chunk boundary splits

**Addresses:**
- Must-have: "Latency reflects full stream duration" (capture in completion task)
- Should-have: "Time-to-first-token metric" (if included)
- Should-have: "Stream completion status tracking" (if included)
- Should-have: "Trailing SSE cost event" (if included)
- Pitfall 3: INSERT-then-UPDATE race (await INSERT or use single-write pattern)
- Pitfall 4: Providers without usage support (handle NULL gracefully)
- Pitfall 5: [DONE] handling (trigger on stream end, not just [DONE])

**Avoids:**
- Pitfall 11: Token counting by deltas (use provider-reported only)

**Research flag:** SKIP—integration of existing components. Standard Rust ownership wrangling (Send + 'static for closure captures). Compiler guides the wiring.

### Phase Ordering Rationale

- **Phase 1 first** because `stream_options` injection is the feature enabler. Without it, providers may not send usage data, making all downstream extraction futile. This phase is also fully backward compatible (additive field, optional injection) and can be tested independently.

- **Phase 2 isolated** because the stream wrapper is the highest-complexity component. Building it as a standalone module with comprehensive unit tests de-risks the integration. The SseLineBuffer and parse_sse_usage functions are pure logic testable without handlers, databases, or HTTP.

- **Phase 3 last** because it depends on both Phase 1 (request mutation) and Phase 2 (stream wrapper). Integration tests verify the complete flow: request → inject → provider → extract → update. This phase has the highest "looks done but isn't" risk, so the checklist from Pitfalls research applies here.

**Dependency chain:** Phase 1 and Phase 2 are independent (can be built in parallel). Phase 3 requires both Phase 1 and Phase 2 complete. Total estimated LOC: ~240 (table stakes only), ~315 (with differentiators).

### Research Flags

Phases likely needing deeper research during planning:
- **Phase 2 (SSE parsing)**: REVIEW recommended. While SSE is well-documented, chunk boundary buffering has subtle edge cases (multi-byte UTF-8 split, pathological fragmentation). Consider reading `eventsource-stream` source (crates.io: 271K/mo downloads) for buffering strategy validation without adding the dependency. Test plan should include adversarial chunk splitting.

Phases with standard patterns (skip research-phase):
- **Phase 1 (types and DB)**: Well-understood. Serde field addition, SQL UPDATE by indexed column, fire-and-forget spawn pattern already exists in logging.rs.
- **Phase 3 (integration)**: Mechanical wiring. Compiler enforces Send + 'static. Integration test coverage verifies correctness.

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| Stack | HIGH | Zero new dependencies, all tools already in Cargo.toml. Verified against existing code (handlers.rs stream wrapper already uses .map(), retry.rs already uses Arc<Mutex> pattern). |
| Features | HIGH | OpenAI `stream_options` API documented officially. LiteLLM, OpenRouter, vLLM all document streaming usage patterns. Table stakes vs differentiators clear from ecosystem comparison. |
| Architecture | HIGH | Insert-then-update pattern preserves current safety property. Tee architecture via .map() already exists in codebase. Post-stream notification via oneshot is standard Tokio pattern. |
| Pitfalls | HIGH | Chunk boundary splits verified via community reports and Rust forum discussions. Provider compatibility verified via OpenAI docs, OpenRouter docs, Groq GitHub issues. All critical pitfalls have known prevention strategies. |

**Overall confidence:** HIGH

### Gaps to Address

**Provider compatibility verification**: While `stream_options` is documented for OpenAI/vLLM/Ollama, Routstr marketplace provider support is unknown. Gap resolution: implement with safe degradation (NULL usage if unsupported) and validate against real Routstr providers in Phase 3 integration testing. If major providers don't support it, add per-provider config flag (`inject_stream_usage: bool`) in a follow-up phase.

**Error stream handling**: Research focused on successful streaming responses. Error responses (provider returns 500, or mid-stream error chunk) need explicit handling. Gap resolution: Phase 3 should include tests for (1) non-200 upstream responses (already handled before streaming starts), (2) mid-stream error chunks with `finish_reason: "error"`, (3) connection drops. Ensure error streams don't crash the parser.

**Performance impact of per-chunk parsing**: Research identified the overhead but didn't benchmark. Gap resolution: Phase 2 implementation should include substring check (`line.contains("\"usage\"")`) before JSON parse. Phase 3 should add benchmark comparing TTFB and throughput with/without interception. If impact is measurable (>5ms TTFB increase), optimize in a follow-up.

**Multi-model tokenizer differences**: If client-side token counting is ever needed (as fallback when providers don't send usage), different models use different tokenizers (GPT-4o uses cl100k_base, Claude uses custom). Gap resolution: explicitly decide NOT to implement client-side counting for v1.2. Log NULL when usage unavailable. Defer tokenizer fallback to future milestone if Routstr provider testing shows it's necessary.

## Sources

### Primary (HIGH confidence)
- arbstr codebase analysis: src/proxy/handlers.rs (streaming logic lines 665-729), src/proxy/types.rs (request struct), src/storage/logging.rs (fire-and-forget pattern), src/proxy/retry.rs (Arc<Mutex> precedent), Cargo.toml + Cargo.lock (dependency verification)
- [OpenAI Chat Streaming API reference](https://platform.openai.com/docs/api-reference/chat-streaming): `stream_options`, `include_usage`, ChatCompletionChunk schema with usage field
- [OpenAI streaming usage stats announcement](https://community.openai.com/t/usage-stats-now-available-when-using-streaming-with-the-chat-completions-api-or-completions-api/738156): stream_options.include_usage feature, final chunk format with empty choices
- [OpenAI streaming packets bug report](https://community.openai.com/t/bug-streaming-packets-changed/460882): Documents real-world SSE chunk splitting, confirms need for line buffering
- arbstr Phase-02 Research (`.planning/phases/02-request-logging/02-RESEARCH.md`): SSE chunk parsing patterns, Arc<Mutex> usage capture, identified pitfalls

### Secondary (MEDIUM confidence)
- [vLLM OpenAI-Compatible Server docs](https://docs.vllm.ai/en/stable/serving/openai_compatible_server/): stream_options support, continuous_usage_stats feature
- [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility): Lists stream_options as supported parameter
- [OpenRouter streaming docs](https://openrouter.ai/docs/api/reference/streaming): SSE comments, mid-stream errors, usage in final message
- [LiteLLM token usage tracking](https://docs.litellm.ai/docs/completion/token_usage): Documents streaming cost tracking, known issues with inflated prompt_tokens during client-side estimation
- [Adam Chalmers: Static streams for faster async proxies](https://blog.adamchalmers.com/streaming-proxy/): Rust stream proxy patterns, tee architecture
- [eventsource-stream on lib.rs](https://lib.rs/crates/eventsource-stream): 271K/mo downloads, Event struct API, cross-chunk buffering implementation reference
- [tokio oneshot channel docs](https://docs.rs/tokio/latest/tokio/sync/oneshot/index.html): Single-value notification pattern
- [futures StreamExt docs](https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html): map(), then() combinators
- [axum Body::from_stream docs](https://docs.rs/axum/latest/axum/body/struct.Body.html): Stream type requirements (Send + 'static)
- [Tokio framing tutorial](https://tokio.rs/tokio/tutorial/framing): Byte stream buffering patterns
- [Rust forum: reqwest byte stream line buffering](https://users.rust-lang.org/t/tokio-reqwest-byte-stream-to-lines/65258): Chunk boundary handling discussion

### Tertiary (LOW confidence, needs validation)
- Groq streaming usage via x_groq.usage: Mentioned in community discussions and GitHub issues, not official documentation
- Azure OpenAI stream_options API version requirements: Mentioned in compatibility notes, specific versions not verified
- Routstr provider OpenAI compatibility level: Assumed based on marketplace description, needs runtime verification

---
*Research completed: 2026-02-15*
*Ready for roadmap: yes*
