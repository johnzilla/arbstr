# Stack Research

**Domain:** Heuristic prompt complexity scoring and tier-aware LLM routing (additions to existing Rust proxy)
**Researched:** 2026-04-08
**Confidence:** HIGH

## Recommended Stack Additions

### Core Technologies

| Technology | Version | Purpose | Why Recommended |
|------------|---------|---------|-----------------|
| `regex` | 1.12 | Pattern matching for code blocks, multi-file references, reasoning keywords | Already a transitive dependency via `tracing-subscriber`'s env-filter -- adding as direct dep costs zero additional compile time. Rust's regex crate guarantees linear-time matching, critical for a hot path that runs on every request. |

### Supporting Libraries

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| None needed | -- | -- | -- |

### Development Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| `cargo bench` (built-in) | Benchmark scorer latency | Ensure scorer stays under 1ms per request. Use `criterion` later if more precision needed. |

## What We Already Have (No New Deps Needed)

The existing stack covers most of what the complexity scorer needs:

| Existing Dep | Use for Complexity Scoring |
|-------------|---------------------------|
| `serde` | Deserialize complexity config (signal weights, thresholds) from TOML |
| `toml` | Parse `[complexity]` config section |
| `tracing` | Debug logging of per-signal scores and final tier classification |
| `sqlx` | New `complexity_score` REAL and `tier` TEXT columns in `requests` table |
| `dashmap` | Already used for circuit breakers, no new need |

## Installation

```bash
# Add regex as direct dependency (already transitive, just making it explicit)
cargo add regex@1.12
```

That is the only `cargo add` needed. **Net new transitive dependencies: zero.**

## Alternatives Considered

| Recommended | Alternative | When to Use Alternative |
|-------------|-------------|-------------------------|
| `regex` (direct dep) | `str::contains()` chains | Only if all patterns are simple literal strings. Current keyword matching uses `contains()`, which works for exact keywords but cannot handle code fence detection (``` with language tags), file path patterns, or word-boundary-aware keyword matching. |
| `prompt.len() / 4` for token approximation | `tiktoken-rs` 0.10.0 for exact counts | Only if you later need pre-request cost estimation with less than 10% error. Current use case (tier bucketing into local/standard/frontier) tolerates a 2x error margin. |
| `std::sync::LazyLock` for compiled regex | `once_cell::sync::Lazy` | If targeting Rust editions before 1.80. arbstr uses edition 2021 with modern toolchain, so `LazyLock` (stable since Rust 1.80, stdlib) is preferred. |
| Custom heuristic scorer | ML classifier via `linfa` | Only if heuristic accuracy proves insufficient after real-world data. Explicitly out of scope per PROJECT.md. |

## What NOT to Use

| Avoid | Why | Use Instead |
|-------|-----|-------------|
| `tiktoken-rs` 0.10.0 / `tiktoken` 3.1.2 / `bpe` | Pulls BPE vocabulary data files, adds ~50ms cold-start latency loading tokenizer models. Overkill for a heuristic that just needs "is this prompt roughly 100, 1000, or 10000 tokens." The scorer runs synchronously on every request and must be sub-millisecond. | `prompt.len() / 4` (bytes divided by 4) gives close-enough approximation for English text at zero cost. Actual token count comes back from the provider response. |
| `rust_readability` / `text_analysis` / `Rust_Grammar` | These compute Flesch-Kincaid, Coleman-Liau, and similar readability indices designed for prose. LLM prompt complexity is fundamentally different -- "implement quicksort in Rust" (5 words) is high complexity despite being maximally "readable." | Custom signal-based scorer with regex patterns for structural features (code blocks, file paths, reasoning keywords). |
| `lazy-regex` 3.6.0 | The `regex` crate with `std::sync::LazyLock` provides the same lazy initialization without an additional dependency. arbstr compiles patterns at startup and stores them in AppState, so the macro approach adds no benefit. | `regex::Regex::new()` at init time, stored in scorer struct. |
| `unicode-segmentation` 1.13.2 | Word boundary detection is unnecessary. The scorer uses regex for structural signals and byte length for size estimation. Unicode word counting adds latency with zero benefit to routing accuracy. | `\b` word boundaries in regex patterns cover keyword matching. |
| `once_cell` | `std::sync::LazyLock` covers the same use case and is in stdlib since Rust 1.80. No reason to add a dependency for what the standard library provides. | `std::sync::LazyLock` |

## Integration Points

### Where regex fits in the codebase

The complexity scorer should be a new module (`src/scorer.rs` or `src/complexity.rs`) that:

1. **Receives** the full `ChatCompletionRequest` (messages array, model, any metadata)
2. **Computes** individual signal scores using regex patterns and arithmetic
3. **Returns** a `ComplexityResult { score: f32, tier: Tier, signals: SignalBreakdown }`
4. **Is called** from the handler before routing, result passed to router for tier-aware selection

Regex patterns should be compiled once at startup and stored in the scorer struct (inside `AppState`). Pattern compilation is expensive (~microseconds); matching is fast (~nanoseconds per pattern on typical prompt lengths).

### Signal implementation mapping

| Signal | Implementation | Library Needed |
|--------|---------------|----------------|
| Context length | `messages.iter().map(\|m\| m.content.len()).sum::<usize>() / 4` | None (stdlib) |
| Code block count | `Regex::new(r"```")` count matches in content | `regex` |
| Multi-file references | `Regex::new(r"(?:src/\|\.rs\b\|\.py\b\|\.ts\b\|\.js\b\|\.go\b)")` count matches | `regex` |
| Reasoning keywords | `Regex::new(r"\b(?:analyze\|compare\|design\|architect\|trade-?off\|explain why)\b")` | `regex` |
| Conversation depth | `messages.len()` | None (stdlib) |
| System prompt presence | `messages.iter().any(\|m\| m.role == "system")` | None (stdlib) |
| Header override | Check `X-Arbstr-Tier` header value | None (axum extractors) |

### Config integration

New TOML section, parsed by existing `serde` + `toml` stack:

```toml
[complexity]
# Tier thresholds (score ranges, 0.0 to 1.0)
local_max = 0.3
standard_max = 0.7
# Anything above standard_max routes to frontier

# Signal weights (normalized internally, relative magnitudes matter)
[complexity.weights]
context_length = 0.25
code_blocks = 0.20
multi_file = 0.15
reasoning_keywords = 0.15
conversation_depth = 0.15
system_prompt = 0.10
```

### Provider config addition

```toml
[[providers]]
name = "local-llama"
tier = "local"       # NEW FIELD: local | standard | frontier
# ...existing fields...
```

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| `regex` 1.12 | Rust 1.65+ (MSRV) | arbstr uses edition 2021, well above MSRV |
| `regex` 1.12 | `tracing-subscriber` 0.3 | Already uses `regex` 1.12.3 transitively, no version conflict possible |
| `std::sync::LazyLock` | Rust 1.80+ | Stable stdlib, preferred over `once_cell` for new code |

## Cargo.toml Change

```toml
[dependencies]
# ... existing deps unchanged ...

# Text analysis
regex = "1.12"
```

**Net dependency change: +1 direct dep, +0 transitive deps** (regex and regex-automata already in the tree via tracing-subscriber).

## Sources

- [regex crate on crates.io](https://crates.io/crates/regex) -- v1.12.3 latest, verified via `cargo search`
- [tiktoken-rs on GitHub](https://github.com/zurawiki/tiktoken-rs) -- v0.10.0, evaluated and rejected (HIGH confidence)
- [bpe crate on crates.io](https://crates.io/crates/bpe) -- fast BPE alternative, also rejected for same reasons (MEDIUM confidence)
- [Rust text processing crates on lib.rs](https://lib.rs/text-processing) -- surveyed ecosystem, no suitable crate for LLM prompt complexity (MEDIUM confidence)
- `cargo tree` output -- confirmed `regex` 1.12.3 already transitive via `tracing-subscriber` (HIGH confidence)
- `cargo search` -- verified current versions of all evaluated crates (HIGH confidence)

---
*Stack research for: arbstr v1.7 complexity scoring additions*
*Researched: 2026-04-08*
