# Phase 19: Handler Integration and Escalation - Context

**Gathered:** 2026-04-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Wire the complexity scorer into the request handler, add X-Arbstr-Complexity header override, and implement automatic tier escalation when circuit breakers block the scored tier. This is where scoring meets the live request path.

</domain>

<decisions>
## Implementation Decisions

### Scoring call site
- **D-01:** Score the request inside `resolve_candidates()`. This function already calls `select_candidates` and is shared by both streaming and non-streaming paths.
- **D-02:** `resolve_candidates` needs access to `&[Message]` from the request body (for scoring) and `&RoutingConfig` from `AppState.config` (for weights and thresholds).
- **D-03:** Call `score_complexity(&request.messages, &config.routing.complexity_weights)` to get the score, then `score_to_max_tier(score, config.routing.complexity_threshold_low, config.routing.complexity_threshold_high)` to get `max_tier`.
- **D-04:** Pass the computed `Some(max_tier)` to `select_candidates` instead of `None`.

### Escalation loop
- **D-05:** Escalation happens inside `resolve_candidates`. If `select_candidates` returns `NoPolicyMatch` with a tier filter active, try the next tier up.
- **D-06:** Escalation order: `Local → Standard → Frontier`. Maximum 2 escalation attempts per request.
- **D-07:** One-way only -- never de-escalate. Once escalated, the expanded tier is final for the request.
- **D-08:** Reuse existing circuit breaker state -- `select_candidates` already gets filtered candidates; circuit breaker filtering happens in the handler's retry loop. Escalation expands the candidate pool before retry.
- **D-09:** Log escalation at WARN level: "Tier escalation: {from_tier} → {to_tier} (no healthy providers at {from_tier})"

### Header override
- **D-10:** Parse `X-Arbstr-Complexity` header from request. Case-insensitive value matching.
- **D-11:** Valid values: `high` → `Tier::Frontier`, `medium` → `Tier::Standard`, `low` → `Tier::Local`.
- **D-12:** Invalid or missing header → fall through to scorer (no error, no warning).
- **D-13:** Header override skips the scorer entirely -- the header value IS the max_tier, no scoring needed.
- **D-14:** Header is checked before scoring so we skip the scorer computation when override is present.

### Claude's Discretion
- Whether `resolve_candidates` signature needs to change or if it accesses messages/config through existing params
- Exact error matching for NoPolicyMatch in escalation loop
- Whether to add a dedicated `NoTierMatch` error variant or reuse `NoPolicyMatch`

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Handler (primary modification target)
- `src/proxy/handlers.rs` line ~240 -- `resolve_candidates()` function, currently passes `None` to `select_candidates`
- `src/proxy/handlers.rs` -- `chat_completions` handler that calls `resolve_candidates`

### Router (consumed APIs)
- `src/router/complexity.rs` -- `score_complexity()` and `score_to_max_tier()` functions
- `src/router/selector.rs` -- `select_candidates(model, policy, prompt, max_tier)` with `Option<Tier>`

### Config
- `src/config.rs` -- `RoutingConfig` with thresholds and weights, `ComplexityWeightsConfig`

### Types
- `src/proxy/types.rs` -- `ChatCompletionRequest` with `messages: Vec<Message>`

### Research (escalation design)
- `.planning/research/ARCHITECTURE.md` -- escalation is handler concern, not router concern
- `.planning/research/PITFALLS.md` -- one-way escalation prevents retry cycles, vault reservation at frontier price

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `resolve_candidates` already has access to `&AppState` (which contains config and router)
- `RequestContext` carries header info -- X-Arbstr-Policy already parsed as a pattern
- Circuit breaker filtering happens in the handler's retry loop via `candidates.iter().filter(|c| !breakers.is_open(&c.name))`

### Established Patterns
- Headers parsed via `headers.get("x-arbstr-policy")` pattern in handlers.rs
- Error responses use `error_response()` helper returning OpenAI-compatible JSON
- `resolve_candidates` returns `Result<ResolvedCandidates, Response>` -- errors become HTTP responses

### Integration Points
- `resolve_candidates` is called from `chat_completions` handler
- The retry loop in `chat_completions` iterates over candidates from `resolve_candidates`
- Circuit breaker state checked per-candidate in the retry loop

</code_context>

<specifics>
## Specific Ideas

No specific requirements -- open to standard approaches.

</specifics>

<deferred>
## Deferred Ideas

None -- discussion stayed within phase scope.

</deferred>

---

*Phase: 19-handler-integration-and-escalation*
*Context gathered: 2026-04-08*
