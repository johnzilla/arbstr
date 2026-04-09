# Phase 17: Complexity Scorer - Context

**Gathered:** 2026-04-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Implement a pure heuristic complexity scorer that takes the full conversation messages array and returns a 0.0-1.0 score. The scorer uses 5 weighted signals (context length, code blocks, multi-file indicators, reasoning keywords, conversation depth) configured via `ComplexityWeightsConfig` from Phase 16. No routing integration -- tier selection happens in Phase 18.

</domain>

<decisions>
## Implementation Decisions

### Signal calibration
- **D-01:** Context length signal uses a sigmoid curve centered at ~4K chars (~1K tokens). Smooth transition from low to high scores.
- **D-02:** Token approximation uses `total_chars / 4` -- no tokenizer library. Sufficient for tier bucketing.
- **D-03:** Code block detection uses regex for fenced code blocks (triple backtick patterns).
- **D-04:** Multi-file detection uses regex for file path patterns (e.g., `src/`, `.rs`, `.py`, `/path/to/`) across messages.
- **D-05:** Reasoning keywords: hardcoded default set + `extra_keywords` config field on `ComplexityWeightsConfig`. Users can ADD keywords but can't remove defaults. Default set includes: "architect", "design", "tradeoff", "refactor", "why does", "compare", "across the codebase", "step by step", "analyze", "evaluate", "debug".
- **D-06:** Conversation depth signal based on `messages.len()`. Sigmoid or linear curve -- deeper conversations score higher.
- **D-07:** Use `std::sync::LazyLock` (stable since Rust 1.80) for compiled regex patterns at module level. No `once_cell` or `lazy-regex` crate needed.

### Scoring formula
- **D-08:** Final score = weighted average: `Sum(signal_i * weight_i) / Sum(weight_i)`, clamped to [0.0, 1.0].
- **D-09:** Weights come from `ComplexityWeightsConfig` (already parsed in Phase 16 config).
- **D-10:** Default-to-frontier: if total message content is < 10 chars or messages array is empty, return 1.0 immediately. Never route garbage/empty to local.

### Module placement
- **D-11:** New file `src/router/complexity.rs` for all scoring logic.
- **D-12:** Public function `score_complexity(messages: &[Message], weights: &ComplexityWeightsConfig) -> f64`.
- **D-13:** Re-export from `src/router/mod.rs` so handlers can call `crate::router::score_complexity`.
- **D-14:** Add `regex` as direct dependency in Cargo.toml (already a transitive dep via tracing-subscriber).

### Claude's Discretion
- Exact sigmoid curve parameters (midpoint, steepness)
- Individual signal normalization ranges
- Whether to expose individual sub-scores in a debug struct (for future /v1/classify endpoint)
- Exact reasoning keyword list beyond the core set specified in D-05

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Config types (from Phase 16)
- `src/config.rs` lines 182-245 -- `RoutingConfig`, `ComplexityWeightsConfig` structs with all 5 signal weight fields
- `src/config.rs` lines 149-180 -- `Tier` enum with ordering

### Message types
- `src/proxy/types.rs` lines 11-48 -- `ChatCompletionRequest`, `Message`, `MessageContent` (string or content-part array)

### Research
- `.planning/research/STACK.md` -- regex 1.12 recommendation, tiktoken-rs rejection rationale, LazyLock for compiled patterns
- `.planning/research/FEATURES.md` -- heuristic signal design, NVIDIA classifier dimensions, weighted-sum validation
- `.planning/research/PITFALLS.md` -- false-negative risks, conversation context design, default-to-frontier principle

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `ComplexityWeightsConfig` with 5 signal weight fields (context_length, code_blocks, multi_file, reasoning_keywords, conversation_depth) -- all default 1.0
- `Message.content` is `MessageContent` enum -- need to handle both `String` and `Array` variants when extracting text
- Existing `MessageContent` in types.rs already handles string/array content parts

### Established Patterns
- Router module uses `mod.rs` + `selector.rs` structure -- add `complexity.rs` alongside
- Pure functions preferred -- `score_complexity` takes references, no side effects
- Config structs are `Debug + Clone + Deserialize` -- weights passed by reference

### Integration Points
- Scorer is called from handler in Phase 19 (not this phase)
- Scorer consumes `&[Message]` from `ChatCompletionRequest.messages`
- Scorer consumes `&ComplexityWeightsConfig` from `config.routing.complexity_weights`

</code_context>

<specifics>
## Specific Ideas

- Research validated 5 signals as covering the complexity space well (NVIDIA classifier uses similar dimensions)
- `len/4` token approximation is well-known and sufficient for tier bucketing -- exact token counts come from provider responses
- Regex patterns compiled once at module level via `LazyLock` -- zero per-request allocation
- Function must be fast: sub-millisecond, no external calls, pure heuristics

</specifics>

<deferred>
## Deferred Ideas

None -- discussion stayed within phase scope.

</deferred>

---

*Phase: 17-complexity-scorer*
*Context gathered: 2026-04-08*
