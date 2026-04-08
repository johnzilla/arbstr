# Pitfalls Research

**Domain:** Adding prompt complexity classification and tier-aware routing to an existing Rust LLM proxy
**Researched:** 2026-04-08
**Confidence:** HIGH (based on direct codebase analysis of selector.rs, handlers.rs, circuit_breaker.rs, retry.rs, config.rs, stream.rs; common patterns in LLM routing systems; heuristic classification pitfalls from search/ranking systems)

## Critical Pitfalls

### Pitfall 1: False Negatives Route Complex Prompts to Local Models, Producing Garbage

**What goes wrong:**
The complexity scorer misclassifies a complex prompt as simple. A multi-step reasoning request, a nuanced code refactoring task, or a long conversation with implicit context gets a low score and routes to a local/mesh model (e.g., a 7B parameter model). The local model produces confidently wrong output. The user sees a plausible-looking but incorrect response and does not realize the routing was wrong.

This is strictly worse than the current behavior (always route to frontier). A bad response from a cheap model costs more than an expensive response from a frontier model because the user wastes time acting on wrong output before discovering the error.

**Why it happens:**
Heuristic complexity scoring has inherent blind spots. Short prompts can be deeply complex ("Prove P != NP"), while long prompts can be trivially simple (a wall of text asking "summarize this"). Context-dependent complexity is invisible to heuristics: "fix this bug" is simple if the bug is a typo, complex if it requires architectural reasoning, but the prompt text is identical. Conversation history adds context that a single-message scorer misses -- the 10th message in a debugging session carries context from the previous 9.

Developers building the scorer test with obvious cases ("hello" = simple, "implement a distributed consensus algorithm" = complex) and ship when those pass. The failure mode is in the gray zone, which is where most real-world prompts live.

**How to avoid:**
1. Default to FRONTIER, not local. The scorer should identify prompts that are OBVIOUSLY simple and route those down. Unknown/ambiguous prompts must default to the highest tier. This means false negatives (complex classified as simple) require the scorer to be actively wrong, not just uncertain.
2. Implement a confidence threshold on the complexity score. If the scorer is not confident the prompt is simple (score below threshold by a wide margin), route to frontier. Only route to local when the score is well below the simple/standard boundary.
3. The `X-Arbstr-Tier` header override (already in the feature list) is the escape hatch. Document it prominently. Users who notice bad responses from local routing can force frontier.
4. Log the complexity score, individual signal contributions, and selected tier for every request. This creates a dataset for tuning thresholds without requiring the user to report every misroute.
5. Start with an aggressively conservative scorer: only route to local for prompts that hit MULTIPLE simple signals (short, no code blocks, no reasoning keywords, shallow conversation). A single signal should not be sufficient.

**Warning signs:**
- Users manually setting `X-Arbstr-Tier: frontier` on most requests (the heuristic is not saving them money, it is costing them time)
- Local model response quality complaints correlate with specific prompt patterns
- The "simple" tier handles less than 10% of traffic (thresholds are too conservative, but this is the SAFE direction)

**Phase to address:**
Complexity scorer implementation phase. The default-to-frontier principle must be the first design constraint, before any signal weights are tuned.

---

### Pitfall 2: Scoring the Wrong Content -- Missing Conversation Context

**What goes wrong:**
The scorer examines only the latest user message in a multi-turn conversation. A 10-message debugging session where the user finally says "try again" scores as trivially simple (short message, no code, no reasoning keywords). The local model has no idea what "try again" means in context because it lacks the conversation history, and even if history is forwarded, the local model lacks the capacity to reason over it.

The inverse also fails: scoring the ENTIRE conversation history (all messages concatenated) heavily biases toward "complex" because any multi-turn conversation accumulates length, code blocks, and keywords. This defeats the purpose -- everything routes to frontier after 3 messages.

**Why it happens:**
The existing `find_policy` in selector.rs (line 133-158) already operates on a single `prompt: Option<&str>` -- the user message. Developers naturally extend this pattern for complexity scoring, examining the same single string. The ChatCompletionRequest has a `messages` array but only the last user message is extracted for policy matching today.

**How to avoid:**
1. Score with awareness of conversation depth. The number of messages in the conversation is itself a complexity signal: `messages.len() > 4` should bias toward standard/frontier regardless of the last message content.
2. For the last user message, check if it is a short follow-up (under ~20 tokens, no code blocks). Short follow-ups in deep conversations are continuation prompts -- they inherit the complexity of the conversation, not their own text.
3. Score the system message separately if present. System messages often contain the actual task complexity ("You are an expert Rust developer reviewing code for safety issues") while user messages are short inputs.
4. Do NOT concatenate all messages for scoring. Instead, score the last user message AND apply conversation-depth multipliers. This keeps scoring fast while capturing context.
5. The scorer function signature should accept `&[Message]` (the full messages array), not just the prompt string. This is a signature-level decision that is hard to change later.

**Warning signs:**
- Multi-turn conversations consistently route to local after the first complex message
- Users report "it was working fine, then the quality dropped mid-conversation"
- The complexity score for message N has no correlation with the score for message N-1 in the same conversation

**Phase to address:**
Complexity scorer design phase. The function signature (`&[Message]` vs `&str`) must be decided before implementation.

---

### Pitfall 3: Complexity Scoring Adds Latency to Every Request's Hot Path

**What goes wrong:**
The complexity scorer runs synchronously in the request path, between receiving the request and selecting a provider. If scoring involves any non-trivial computation -- regex matching against multiple patterns, tokenization for length estimation, iterating over all messages -- it adds latency to every request. For streaming requests (the common case with LLM usage), this latency is felt as time-to-first-token delay, which users are extremely sensitive to.

The existing routing path is fast: `select_candidates` does a linear scan of providers (typically 2-5) with string comparisons. Adding a scorer that does regex matching over potentially large prompts (some code-generation prompts include entire files, 10k+ tokens) changes the performance profile.

**Why it happens:**
Developers build the scorer, benchmark it on short test prompts (fast), and do not test with production-length inputs. A regex scan over a 50-token prompt takes microseconds. The same regex scan over a 10,000-token prompt with code blocks takes milliseconds. Multiply by the number of signals being checked.

**How to avoid:**
1. Set a latency budget for scoring: 1ms maximum. Measure it. Add a tracing span around the scorer so latency is visible in logs.
2. Design signals to be O(1) or O(n) with early termination:
   - Message count: O(1)
   - Last message length: O(1) -- check byte length, not token count
   - Code block detection: scan for triple backticks, stop at first match
   - Keyword matching: use a simple `contains()` check against a small keyword set, not regex
3. Do NOT tokenize the input for "accurate" length estimation. Byte length divided by 4 is a good-enough proxy for token count. Actual tokenization (even with tiktoken) is expensive and unnecessary for heuristic routing.
4. If the prompt exceeds a size threshold (e.g., 32KB), short-circuit to "complex" without scanning the full content. Large prompts are almost always complex.
5. Keep the scorer synchronous (no async). It should be a pure function: `fn score(messages: &[Message], config: &ScorerConfig) -> ComplexityScore`. No I/O, no allocations beyond a small score struct.

**Warning signs:**
- Time-to-first-token increases after deploying complexity routing
- Scoring latency appears in traces for large requests
- The scorer allocates strings (e.g., `to_lowercase()` on the full prompt) instead of scanning in-place

**Phase to address:**
Scorer implementation phase. The latency budget should be a design constraint, with a benchmark test that fails if scoring exceeds 1ms on a 10k-token input.

---

### Pitfall 4: Tier Escalation on Circuit Break Creates Infinite Retry Loops

**What goes wrong:**
The feature spec says "automatic escalation on circuit break." When a local-tier provider's circuit opens, the system escalates to standard or frontier. But the escalation interacts with the existing retry-with-fallback logic in unexpected ways.

Scenario: local provider circuit is open. Request escalates to standard tier. Standard provider returns 500. Retry logic kicks in with backoff. Standard provider fails again. Fallback to next candidate. If the next candidate is a frontier provider, good. But if the escalation logic re-evaluates the tier and the circuit breaker state has changed (local circuit timed out to half-open), the system might de-escalate back to local, which fails again, re-opens the circuit, and re-escalates to standard. The request bounces between tiers.

**Why it happens:**
The existing retry loop in retry.rs is designed for a flat candidate list sorted by cost. It retries on the primary, then falls back to the next candidate. Tier escalation adds a SECOND dimension to fallback: not just "next provider at this tier" but "next tier entirely." If both dimensions are active simultaneously, the fallback path becomes a graph instead of a list, and cycles are possible.

**How to avoid:**
1. Tier escalation must be a ONE-WAY gate. Once a request escalates from local to standard, it NEVER de-escalates back to local within the same request. This prevents cycles.
2. Implement escalation as candidate list expansion, not re-routing. When the local tier is unavailable (all circuits open), append standard-tier candidates to the candidate list. When standard is also unavailable, append frontier. The retry loop then works on this expanded list linearly, same as today.
3. The candidate list order should be: local (if circuits allow) -> standard -> frontier. The retry loop picks from this list in order. Circuit-open candidates are filtered out before the list is built (as currently done in `resolve_candidates`).
4. Do NOT re-evaluate complexity or tier selection during retries. The tier decision is made once, at request entry, and the candidate list is built from that decision. Retries only move through the pre-built list.
5. Add a `x-arbstr-escalated: true` response header and log the original tier vs actual tier in the DB. This makes escalation events visible.

**Warning signs:**
- Requests that take the full 30s timeout before failing
- Log entries showing the same request hitting the same provider multiple times with different tier labels
- Escalation rate above 50% (the scorer is not working, everything escalates)

**Phase to address:**
Tier-aware routing pipeline phase. The one-way escalation constraint and candidate-list-expansion approach must be decided before the retry integration is modified.

---

### Pitfall 5: Config Complexity Explosion -- Signal Weights Become Unmaintainable

**What goes wrong:**
The complexity scorer has multiple signals: context length, code blocks, multi-file references, reasoning keywords, conversation depth. Each signal has a weight. Each tier boundary has a threshold. The config ends up looking like:

```toml
[complexity]
weight_context_length = 0.3
weight_code_blocks = 0.2
weight_reasoning_keywords = 0.15
weight_conversation_depth = 0.15
weight_multi_file = 0.2
threshold_local = 0.3
threshold_standard = 0.7
keyword_list = ["analyze", "compare", "implement", "architect", "design", "optimize", "debug", "refactor"]
code_block_multiplier = 1.5
max_context_tokens = 4096
```

Nobody can intuit what changing `weight_code_blocks` from 0.2 to 0.25 does to routing behavior. The interactions between weights are non-obvious. Users do not want to tune 10+ parameters to get reasonable routing.

**Why it happens:**
Developers expose every internal knob as config because "flexibility." Each signal feels like it needs a separate weight because importance varies by use case. The result is a config surface that is technically flexible but practically opaque.

**How to avoid:**
1. Expose ONE user-facing config knob: `complexity_bias` with values like `"aggressive"` (route more to local), `"balanced"` (default), `"conservative"` (route more to frontier). This maps internally to preset weight profiles.
2. Individual signal weights should be hardcoded defaults that are rarely changed. If someone needs to tune them, they can, but the primary interface is the single bias knob.
3. Tier thresholds should be derived from the bias setting, not separately configured. `"aggressive"` = lower thresholds (more goes to local), `"conservative"` = higher thresholds (more goes to frontier).
4. The keyword list should have sensible defaults compiled into the binary. Config should allow ADDING keywords, not replacing the entire list (use `extra_keywords` not `keywords`).
5. Keep the `[complexity]` config section optional. If omitted entirely, use balanced defaults. The zero-config experience should work.

**Warning signs:**
- Users copy-pasting config from GitHub issues because they cannot figure out the right values
- Bug reports that are actually config tuning questions
- The config example file has more comments explaining complexity config than all other sections combined

**Phase to address:**
Config design phase, before the scorer is implemented. The config shape constrains the scorer interface.

---

### Pitfall 6: Tier Field on ProviderConfig Breaks Existing Configs

**What goes wrong:**
Adding a `tier` field to `[[providers]]` in config.toml is a breaking change if the field is required. Every existing config file would fail to parse after upgrading arbstr. Even if the field is optional with a default, the default choice matters: defaulting to `"standard"` means existing providers silently change routing behavior when complexity scoring is added. Defaulting to `"frontier"` means local providers are never used unless explicitly tagged.

**Why it happens:**
The `ProviderConfig` struct (config.rs line 149) uses `#[derive(Deserialize)]`. Adding a required field without `#[serde(default)]` makes deserialization fail on existing configs that lack the field. This is a basic serde compatibility issue but easy to overlook when the developer always tests with new config files.

**How to avoid:**
1. Make `tier` optional with `#[serde(default)]`. Default to `"standard"` -- this means existing providers behave as they do today (eligible for any complexity level, sorted by cost).
2. The tier system should be additive: providers without a tier annotation participate in all tiers. Providers WITH a tier annotation are restricted to that tier and below. This means existing configs get the same routing behavior they have today.
3. The `tier` field should accept values: `"local"`, `"standard"`, `"frontier"`. An unknown value should be a config validation error at startup (fail fast), not silently defaulted.
4. Add a `cargo run -- check` validation that warns when complexity routing is enabled but no providers have explicit tier annotations. This helps users migrate.
5. Write a migration test: deserialize the existing `config.example.toml` and verify it parses correctly with the new tier field absent.

**Warning signs:**
- Users report "arbstr won't start after upgrade" with a deserialization error
- Existing integration tests fail because test configs lack the tier field

**Phase to address:**
Config and provider tier system phase. Must be the first thing implemented, before the scorer, because the scorer needs to know what tiers exist.

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Hardcoded signal weights instead of config | Faster implementation, no config design needed | Cannot tune without recompile; different use cases need different weights | MVP only -- add config in follow-up phase |
| String matching for code blocks instead of proper parsing | Simple implementation, no dependencies | Misses indented code blocks (4-space markdown), false positives on triple-backtick in prose | Always acceptable -- proper parsing is overkill for a heuristic |
| Byte length as token count proxy (bytes/4) | No tokenizer dependency, O(1) | Inaccurate for non-ASCII text (CJK characters are 3 bytes but 1-2 tokens) | Always acceptable -- heuristic routing does not need token-level accuracy |
| Single complexity score (f32) instead of per-signal breakdown in DB | Simpler schema, less storage | Cannot analyze which signals drove routing decisions; harder to debug misroutes | MVP only -- add signal breakdown columns later |
| Threshold hardcoded in scorer instead of per-provider | Simpler routing logic | Cannot have "this local model handles medium complexity but not high" | Acceptable until provider diversity increases |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| Scorer + Router | Scorer returns a tier, router filters by tier, ignoring cost within tier | Scorer returns tier, router filters to that tier AND sorts by cost within tier. Cheapest-within-tier, not just any-within-tier |
| Tier escalation + Circuit breaker | Escalation re-runs the scorer with different parameters | Escalation expands the candidate list to include higher tiers without re-scoring. Score is immutable per request |
| Tier + Vault billing | Reserve amount based on local-tier pricing, then escalate to frontier | Reserve amount must use FRONTIER pricing (worst case) since escalation is possible. Settle with actual cost. Under-reservation causes vault rejections |
| Complexity score + Streaming SSE metadata | Score computed but not included in trailing SSE event | Add `complexity_score` and `tier` to the trailing SSE metadata alongside `cost_sats` and `latency_ms` |
| Tier + Stats endpoint | `group_by=tier` returns tiers but tier is stored as string, not validated | Use an enum column or CHECK constraint in SQLite. Validate tier values at insert time, not query time |
| Header override + Scorer | `X-Arbstr-Tier: local` forces local even when scorer says frontier | Honor the header override absolutely. Log a warning if override contradicts scorer by 2+ tiers. This is the user's escape hatch and must not be second-guessed |

## Performance Traps

| Trap | Symptoms | Prevention | When It Breaks |
|------|----------|------------|----------------|
| Regex-based keyword matching on full prompt | Scoring latency spikes on large prompts (>10k tokens) | Use `str::contains()` with a small keyword set, not compiled regex | Prompts above 5k tokens with 20+ keywords to check |
| Allocating lowercase copy of full prompt for case-insensitive matching | Memory spike, GC pressure (Rust: allocator churn) | Use `to_ascii_lowercase()` on small substrings or byte-level comparison | Prompts with full file contents (50k+ bytes) |
| Scoring every message in conversation history | Latency proportional to conversation length | Score last user message + system message + use message count as signal | Conversations with 50+ messages |
| Logging per-signal scores at DEBUG level with string formatting | Format strings allocated even when DEBUG is disabled | Use `tracing::debug!(signal = %value)` which is lazy | High-throughput proxy with verbose logging config |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Keyword list in config used as prompt content filter | User interprets "reasoning keywords" as content moderation; fails to block anything meaningful | Document clearly that keywords are routing hints, not content filters. They affect cost, not safety |
| Tier override header allows clients to force local routing for expensive operations | Malicious client forces local tier to get free/cheap inference on complex tasks, degrading quality | Not a security issue for single-user proxy. For future multi-user: rate-limit tier overrides or require auth |
| Complexity score leaked in response headers reveals routing logic | Competitors or adversaries learn how to craft prompts that game the routing | Acceptable risk for personal proxy. For production: make score headers opt-in |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| No visibility into WHY a request was routed to a tier | User gets bad response, has no idea it went to local model instead of frontier | Include `x-arbstr-tier` and `x-arbstr-complexity-score` in response headers. Make tier visible in trailing SSE event |
| Complexity thresholds too aggressive by default | First experience after upgrade: quality drops on some requests | Ship with conservative defaults (bias toward frontier). Let users opt into aggressive routing after they trust the scorer |
| No way to see routing decisions without making a request | User cannot predict where a prompt will route | Add a dry-run endpoint: `POST /v1/classify` that returns the complexity score and tier without executing the request |
| Tier names are opaque ("local", "standard", "frontier") | User does not know which actual model will serve the request | Include provider name AND tier in response headers. "Your request went to provider-alpha (local tier)" |
| Silent escalation with no user signal | Request takes longer than expected because it escalated from local to standard after circuit break, but user does not know | Add `x-arbstr-escalated: true` header and `escalated_from` field in DB log |

## "Looks Done But Isn't" Checklist

- [ ] **Complexity scorer:** Often missing conversation depth signal -- verify scorer accepts `&[Message]` not just `&str`
- [ ] **Tier routing:** Often missing cost sorting within tier -- verify cheapest-within-tier, not random-within-tier
- [ ] **Config backwards compat:** Often missing migration test -- verify existing `config.example.toml` parses without tier field
- [ ] **Escalation:** Often missing one-way constraint -- verify a request never de-escalates within the same request
- [ ] **Vault integration:** Often missing reserve-at-frontier-price -- verify reserve uses worst-case tier pricing when escalation is possible
- [ ] **Stats endpoint:** Often missing `group_by=tier` -- verify tier is a stored DB column, not computed at query time
- [ ] **Streaming:** Often missing tier in SSE metadata -- verify trailing event includes `tier` field alongside `cost_sats`
- [ ] **Response headers:** Often missing `x-arbstr-tier` and `x-arbstr-complexity-score` -- verify both present on streaming and non-streaming responses
- [ ] **DB schema:** Often missing complexity_score column -- verify migration adds `complexity_score REAL` and `tier TEXT` to requests table
- [ ] **Circuit breaker interaction:** Often missing tier-aware filtering in `resolve_candidates` -- verify circuit filtering happens AFTER tier filtering, not before

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| False negatives routing complex to local | LOW | Increase thresholds via config (or set bias to "conservative"). No code change needed if config is properly exposed |
| Conversation context ignored | MEDIUM | Change scorer function signature from `&str` to `&[Message]`. Requires updating all call sites in handlers.rs |
| Config breaks existing users | LOW | Add `#[serde(default)]` to tier field. Patch release. No data migration |
| Infinite retry loop from tier cycling | HIGH | Requires redesign of escalation to use one-way gate pattern. Cannot be fixed with config |
| Scoring latency on hot path | LOW | Add size-based short-circuit (large prompts = complex). Profile and fix specific slow signals |
| Vault under-reservation on escalation | MEDIUM | Change reserve logic to use frontier pricing. Requires understanding vault settlement semantics |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| False negatives (Pitfall 1) | Scorer implementation | Test with ambiguous prompts; verify default is frontier; measure false-negative rate on sample prompt corpus |
| Conversation context (Pitfall 2) | Scorer design | Verify function signature accepts `&[Message]`; test multi-turn conversation routing |
| Scoring latency (Pitfall 3) | Scorer implementation | Benchmark test: 10k-token input scores in <1ms; tracing span around scorer |
| Tier cycling (Pitfall 4) | Routing pipeline design | Test: request that escalates never de-escalates; no candidate appears twice in expanded list |
| Config explosion (Pitfall 5) | Config design | Config example file has <10 lines for complexity section; zero-config works with balanced defaults |
| Breaking existing configs (Pitfall 6) | Config + tier system | Deserialize existing config.example.toml without tier field; all existing tests pass without modification |

## Sources

- Direct codebase analysis: `src/router/selector.rs` (routing logic, `select_candidates`, `find_policy`), `src/proxy/handlers.rs` (request flow, `resolve_candidates`, circuit integration), `src/proxy/circuit_breaker.rs` (circuit state machine, permit model), `src/proxy/retry.rs` (retry loop, backoff, fallback), `src/config.rs` (ProviderConfig, serde deserialization)
- Existing PITFALLS.md from v1.4 milestone (circuit breaker pitfalls, retry interaction patterns)
- LLM routing patterns from production systems: token-based routing heuristics, tier escalation patterns
- Heuristic classification anti-patterns from search/ranking systems: false negative asymmetry, feature interaction complexity, config surface area management

---
*Pitfalls research for: prompt complexity classification and tier-aware routing in arbstr*
*Researched: 2026-04-08*
