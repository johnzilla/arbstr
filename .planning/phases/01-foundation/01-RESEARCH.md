# Phase 1: Foundation - Research

**Researched:** 2026-02-02
**Domain:** Cost calculation correction + request correlation IDs in a Rust/axum proxy
**Confidence:** HIGH

## Summary

Phase 1 fixes two foundational issues in arbstr: (1) the routing cost ranking uses only `output_rate` but should use `output_rate + base_fee`, and the system needs a separate full-formula cost calculation for post-response logging; (2) every proxied request needs a unique correlation ID propagated through tracing spans.

The current codebase already has all necessary dependencies (`uuid` with v4, `tracing`, `tower-http` with trace). The changes are surgical: modify `select_cheapest` to rank by `output_rate + base_fee`, add a cost calculation function for the full formula, and inject a UUID into a custom tracing span via `tower-http`'s `TraceLayer::make_span_with`. No new crates are needed.

**Primary recommendation:** Use `TraceLayer::make_span_with` closure to generate a UUID v4 per request and attach it to a tracing span. For cost, implement two distinct functions: a ranking heuristic (`output_rate + base_fee`) and a post-response actual cost calculator (full formula with real token counts).

## Standard Stack

### Core (already in Cargo.toml -- no additions needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `uuid` | 1.x (features: `v4`) | Generate unique correlation IDs | Already in deps, standard for UUID generation in Rust |
| `tracing` | 0.1.x | Structured logging with span-based context | Already in deps, tokio ecosystem standard |
| `tracing-subscriber` | 0.3.x (features: `env-filter`) | Log formatting with span field output | Already in deps |
| `tower-http` | 0.5.x (features: `cors`, `trace`) | HTTP middleware including `TraceLayer` with `MakeSpan` | Already in deps |

### Supporting (no changes needed)

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `axum` | 0.7.x | HTTP framework, request/response handling | Already the server framework |
| `tokio` | 1.x | Async runtime | Already the runtime |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `make_span_with` closure | `tower-http` `request-id` feature (`SetRequestIdLayer` + `PropagateRequestIdLayer`) | The `request-id` feature provides header-based propagation but requires enabling a new feature flag and adds header propagation we don't need yet (correlation ID is internal per CONTEXT.md decisions). The `make_span_with` approach is simpler, already uses enabled features, and puts the ID directly on the tracing span. |
| `make_span_with` closure | `axum-trace-id` crate | External dependency for something achievable with 5 lines of code using existing deps. |
| Custom `MakeSpan` struct | `make_span_with` closure | Struct approach is more reusable but overkill for a single field addition. Closure is idiomatic for simple cases. |

**No new dependencies needed.** All existing crate versions and features are sufficient.

## Architecture Patterns

### Current Code Structure (unchanged by this phase)

```
src/
├── config.rs            # ProviderConfig has input_rate, output_rate, base_fee (all u64)
├── router/
│   └── selector.rs      # Router::select_cheapest() and SelectedProvider
└── proxy/
    ├── server.rs         # AppState, create_router(), TraceLayer setup
    └── handlers.rs       # chat_completions handler
```

### Pattern 1: Two-Formula Cost Model

**What:** Routing and logging use deliberately different cost calculations.
**When to use:** When the routing decision must be made before output token count is known.

The routing heuristic ranks providers by `output_rate + base_fee` (a proxy for total cost that uses only pre-request information). The logging cost uses the full formula `(input_tokens * input_rate + output_tokens * output_rate) / 1000.0 + base_fee` after the response is received and actual token counts are known.

```rust
// Routing heuristic: used in select_cheapest() for provider ranking
// output_rate is the dominant variable cost, base_fee matters for short requests
fn routing_cost(provider: &ProviderConfig) -> u64 {
    provider.output_rate + provider.base_fee
}

// Actual cost: used AFTER response, with real token counts from usage object
// Returns f64 because fractional sats matter for cost optimization
fn actual_cost(
    input_tokens: u32,
    output_tokens: u32,
    input_rate: u64,
    output_rate: u64,
    base_fee: u64,
) -> f64 {
    (input_tokens as f64 * input_rate as f64
        + output_tokens as f64 * output_rate as f64)
        / 1000.0
        + base_fee as f64
}
```

**Source:** CONTEXT.md locked decisions -- routing uses `output_rate + base_fee`, logging uses full formula.

### Pattern 2: Correlation ID via TraceLayer make_span_with

**What:** Generate a UUID v4 per request and attach it to the tracing span wrapping the entire request lifecycle.
**When to use:** For every inbound HTTP request to the proxy.

```rust
// Source: axum/discussions/2273, tower-http docs
use tower_http::trace::TraceLayer;
use tracing::Level;
use uuid::Uuid;

TraceLayer::new_for_http()
    .make_span_with(|request: &http::Request<axum::body::Body>| {
        let request_id = Uuid::new_v4();
        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %request.uri(),
            request_id = %request_id,
        )
    })
```

All `tracing::info!()`, `tracing::debug!()`, etc. calls within the request handling chain will automatically inherit the `request_id` field from the enclosing span. No manual passing of the ID is needed -- the tracing span system handles propagation through async boundaries.

**Source:** [axum discussion #2273](https://github.com/tokio-rs/axum/discussions/2273), [tower-http TraceLayer docs](https://docs.rs/tower-http/0.5.2/tower_http/trace/struct.TraceLayer.html)

### Pattern 3: Input Token Estimation from Request

**What:** Estimate input token count from the request body for use in the actual cost formula (when provider response doesn't include usage).
**When to use:** Fallback only -- the primary source of token counts is the provider's `usage` object in the response.

The standard approximation is 1 token per 4 characters of English text. This is deliberately imprecise -- it is a fallback for when the provider response lacks a `usage` object (which is uncommon for non-streaming responses).

```rust
// Rough estimate: 1 token ~= 4 characters
fn estimate_input_tokens(messages: &[Message]) -> u32 {
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    (total_chars / 4) as u32
}
```

**Important:** Per CONTEXT.md (Claude's Discretion), the exact method for input token estimation is flexible. The character-based estimate is sufficient for Phase 1 because:
- Non-streaming responses from OpenAI-compatible APIs include `usage.prompt_tokens` (the authoritative value)
- The estimate is only needed as a fallback or for pre-response calculations
- Phase 2 (OBSRV-02) will extract actual token counts from the response `usage` object

### Anti-Patterns to Avoid

- **Changing rate types from u64 to f64 in config:** The rates (`input_rate`, `output_rate`, `base_fee`) are configured as whole sats per 1000 tokens. Keep them as `u64` in config and `ProviderConfig`. The fractional result only emerges from the division-by-1000 in the actual cost formula. Changing the config types would be a disruptive change with no benefit.
- **Making the correlation ID an extension instead of a span field:** Per CONTEXT.md, the ID is for tracing logs. Putting it on the span makes it automatically appear in every log line. Storing it in request extensions is an additional step needed only when it must be read by handlers (Phase 3 for response headers), not Phase 1.
- **Forwarding the correlation ID upstream:** Per CONTEXT.md decision, the ID is internal to arbstr. Do NOT add it as a header on the upstream request to the provider.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| UUID generation | Custom random ID scheme | `uuid::Uuid::new_v4()` | Already in deps, cryptographically random, standard format |
| Per-request tracing span | Manual ID passing through handler args | `TraceLayer::make_span_with` | Span system handles async propagation automatically |
| Span field inheritance | Manually adding `request_id` to every log call | tracing span nesting | Child spans/events inherit parent span fields automatically |

**Key insight:** The tracing span system eliminates the need to thread a request ID through function signatures. Once attached to the span in middleware, it appears in every log event within that request's lifetime without any code changes to handlers or router functions.

## Common Pitfalls

### Pitfall 1: Integer Division Truncation in Cost Formula

**What goes wrong:** Using integer arithmetic for `(input_tokens * input_rate + output_tokens * output_rate) / 1000` produces truncated results. For example, 50 input tokens at rate 5 = 250, divided by 1000 = 0 (integer), but should be 0.25 sats.
**Why it happens:** Rust's integer division truncates. The config types are `u64`.
**How to avoid:** Cast to `f64` before the division: `(input_tokens as f64 * input_rate as f64 + ...) / 1000.0 + base_fee as f64`. Return `f64` from the cost function. Per CONTEXT.md decision, costs are stored as REAL (f64), not INTEGER.
**Warning signs:** Cost values that are suspiciously round or zero for small token counts.

### Pitfall 2: Span Level Too Low for Default Log Filter

**What goes wrong:** If the span is created at `Level::DEBUG` but the default log filter is `arbstr=info`, the span fields won't appear in logs.
**Why it happens:** `DefaultMakeSpan` uses `Level::DEBUG`. The current code's default filter is `arbstr=info,tower_http=info`.
**How to avoid:** Use `tracing::info_span!` (not `debug_span!`) for the request span, OR adjust the `tower_http` filter to include debug. Since we want the request_id visible at info level, use `info_span!`.
**Warning signs:** Correlation IDs missing from log output despite being set.

### Pitfall 3: Breaking Existing Test Assertions After Changing Ranking

**What goes wrong:** Tests like `test_select_cheapest` may break if the ranking change (`output_rate` -> `output_rate + base_fee`) changes which provider is "cheapest".
**Why it happens:** Current test data: cheap provider has `output_rate=15, base_fee=0` (total=15), expensive has `output_rate=30, base_fee=1` (total=31). The ranking doesn't change for this data, so existing tests should pass. But any new test data must account for the combined ranking.
**How to avoid:** Verify existing test data still produces same ranking under new formula before writing new tests. Add tests specifically for cases where `base_fee` changes the ranking (e.g., provider A: output_rate=10, base_fee=5 vs provider B: output_rate=14, base_fee=0).
**Warning signs:** `test_select_cheapest` failing after the ranking change.

### Pitfall 4: TraceLayer Ordering in Middleware Stack

**What goes wrong:** If `TraceLayer` is added after other middleware, spans may not wrap the full request lifecycle.
**Why it happens:** axum middleware executes outside-in. Layers added later (lower in code) execute first.
**How to avoid:** Keep `TraceLayer` as the outermost layer (last `.layer()` call on the Router), which is the current position. The existing code already has `.layer(TraceLayer::new_for_http())` as the last layer.
**Warning signs:** Log events from early middleware missing the request_id field.

### Pitfall 5: Not Storing the Request ID for Phase 3 Access

**What goes wrong:** Phase 3 needs to put the correlation ID in a response header (`x-arbstr-request-id`). If the ID only lives in the tracing span, handlers can't easily read it back.
**Why it happens:** Tracing spans are for logging, not for data flow between middleware and handlers.
**How to avoid:** In Phase 1, the ID only needs to be in tracing spans (per requirements). But design the middleware so the UUID can also be stored in request extensions when Phase 3 needs it. For now, just generating it in the span is sufficient. Phase 3 will add extension storage.
**Warning signs:** N/A for Phase 1, but worth noting for forward compatibility.

## Code Examples

### Example 1: Updated select_cheapest with output_rate + base_fee Ranking

```rust
// Source: Derived from CONTEXT.md decision
/// Select the cheapest provider by estimated cost (output_rate + base_fee).
///
/// output_rate is the dominant variable cost. base_fee matters for short
/// requests. input_rate is excluded because output token count is unknown
/// at routing time.
fn select_cheapest<'a>(&self, candidates: &[&'a ProviderConfig]) -> Option<&'a ProviderConfig> {
    candidates
        .iter()
        .min_by_key(|p| p.output_rate + p.base_fee)
        .copied()
}
```

### Example 2: Actual Cost Calculation Function

```rust
// Source: CONTEXT.md locked decision on full formula
/// Calculate the actual cost in sats after a response is received.
///
/// Uses the full formula with real token counts from the provider's
/// usage object. Returns f64 because fractional sats matter for
/// cost optimization (per project decision: no rounding).
pub fn actual_cost_sats(
    input_tokens: u32,
    output_tokens: u32,
    input_rate: u64,
    output_rate: u64,
    base_fee: u64,
) -> f64 {
    (input_tokens as f64 * input_rate as f64
        + output_tokens as f64 * output_rate as f64)
        / 1000.0
        + base_fee as f64
}
```

### Example 3: TraceLayer with Correlation ID

```rust
// Source: tower-http docs, axum discussion #2273
use tower_http::trace::TraceLayer;
use uuid::Uuid;

TraceLayer::new_for_http()
    .make_span_with(|request: &http::Request<axum::body::Body>| {
        let request_id = Uuid::new_v4();
        tracing::info_span!(
            "request",
            method = %request.method(),
            uri = %request.uri(),
            request_id = %request_id,
        )
    })
```

### Example 4: Test for base_fee Affecting Ranking

```rust
// Test that base_fee changes provider ranking
#[test]
fn test_base_fee_affects_cheapest_selection() {
    let providers = vec![
        ProviderConfig {
            name: "low-rate-high-fee".to_string(),
            url: "https://a.example.com/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 5,
            output_rate: 10,
            base_fee: 8, // total routing cost: 18
        },
        ProviderConfig {
            name: "high-rate-no-fee".to_string(),
            url: "https://b.example.com/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 5,
            output_rate: 15,
            base_fee: 0, // total routing cost: 15
        },
    ];

    let router = Router::new(providers, vec![], "cheapest".to_string());
    let selected = router.select("gpt-4o", None, None).unwrap();
    // high-rate-no-fee wins because 15 < 18
    assert_eq!(selected.name, "high-rate-no-fee");
}
```

### Example 5: Test for Actual Cost Calculation

```rust
#[test]
fn test_actual_cost_calculation() {
    // 100 input tokens at 10 sats/1k + 200 output tokens at 30 sats/1k + 1 base_fee
    // = (100*10 + 200*30) / 1000 + 1 = (1000 + 6000) / 1000 + 1 = 7.0 + 1 = 8.0
    let cost = actual_cost_sats(100, 200, 10, 30, 1);
    assert!((cost - 8.0).abs() < f64::EPSILON);

    // Small request: 10 input tokens at 5 sats/1k + 5 output tokens at 15 sats/1k + 0 base_fee
    // = (10*5 + 5*15) / 1000 + 0 = (50 + 75) / 1000 = 0.125
    let cost = actual_cost_sats(10, 5, 5, 15, 0);
    assert!((cost - 0.125).abs() < f64::EPSILON);
}
```

## State of the Art

| Old Approach (current code) | Current Approach (Phase 1 target) | Impact |
|-----|------|--------|
| Ranking by `output_rate` only | Ranking by `output_rate + base_fee` | Providers with high base fees correctly penalized in selection |
| No cost calculation function | `actual_cost_sats()` function with full formula | Foundation for Phase 2 logging with correct cost values |
| No correlation ID | UUID v4 per request in tracing span | Every log line within a request shares the same request_id |
| `TraceLayer::new_for_http()` default | Custom `make_span_with` generating UUID | Correlation ID visible in all structured logs |

**Deprecated/outdated:**
- The current `select_cheapest` doc comment says "by output rate, since that dominates cost" -- this will be updated to reflect the new `output_rate + base_fee` ranking.
- The CLAUDE.md schema defines `cost_sats INTEGER` -- per CONTEXT.md decision this changes to REAL, but that's a Phase 2 concern (schema creation happens in Phase 2).

## Open Questions

1. **Input token estimation method**
   - What we know: Character/4 is the standard rough approximation. Provider responses include `usage.prompt_tokens` for authoritative counts.
   - What's unclear: Whether to implement the estimate function in Phase 1 or defer to Phase 2 when it's actually needed for logging.
   - Recommendation: Implement `actual_cost_sats()` in Phase 1 (it's the foundation), but the function takes token counts as parameters. The estimation or extraction of those counts is Phase 2's job. Phase 1 just needs the function to exist and be tested.

2. **Request ID accessibility for Phase 3**
   - What we know: Phase 3 needs the ID in response headers. The tracing span holds it but isn't directly accessible from handlers.
   - What's unclear: Whether to also store the UUID in request extensions now (forward-looking) or wait for Phase 3.
   - Recommendation: Keep Phase 1 minimal -- UUID in span only. Phase 3 adds extension storage. The `make_span_with` closure is easy to extend later.

## Sources

### Primary (HIGH confidence)
- [tower-http 0.5.2 TraceLayer docs](https://docs.rs/tower-http/0.5.2/tower_http/trace/struct.TraceLayer.html) - make_span_with API
- [tower-http 0.5.2 MakeSpan trait](https://docs.rs/tower-http/0.5.2/tower_http/trace/trait.MakeSpan.html) - trait signature
- [tower-http 0.5.2 request_id module](https://docs.rs/tower-http/0.5.2/tower_http/request_id/index.html) - SetRequestIdLayer, MakeRequestUuid (considered but not recommended)
- [uuid 1.x docs](https://docs.rs/uuid/1/uuid/struct.Uuid.html) - Uuid::new_v4() API
- Local codebase analysis - all source files read and verified
- `cargo test` output - 5 tests pass, confirming current baseline

### Secondary (MEDIUM confidence)
- [axum discussion #2273](https://github.com/tokio-rs/axum/discussions/2273) - Community-verified pattern for request ID in tracing spans, confirmed by axum maintainers
- [tower-http feature flags](https://docs.rs/tower-http/0.5.2/tower_http/index.html) - Verified `request-id` feature exists (but not needed for our approach)
- [tracing span field recording](https://docs.rs/tracing/latest/tracing/struct.Span.html) - `field::Empty` pattern for deferred recording

### Tertiary (LOW confidence)
- Token estimation ratio (1 token ~= 4 chars) - Widely cited approximation, varies by model/tokenizer. Sufficient for fallback estimation, not for billing.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH - All dependencies already in Cargo.toml, no new crates needed, APIs verified against docs
- Architecture: HIGH - Two-formula approach is explicitly locked in CONTEXT.md, span-based correlation ID is the standard axum pattern confirmed by maintainers
- Pitfalls: HIGH - Identified from direct code analysis (integer truncation risk, span level, test compatibility) and verified against existing test data

**Research date:** 2026-02-02
**Valid until:** 2026-03-02 (stable domain -- Rust ecosystem, no fast-moving dependencies)
