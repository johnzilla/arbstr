# Phase 18: Tier-Aware Routing - Context

**Gathered:** 2026-04-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Modify the router to filter providers by tier based on a complexity score. Add `max_tier` parameter to `select_candidates`, add `score_to_max_tier` mapping function, and consume `RoutingConfig` thresholds. No handler integration or escalation logic -- that's Phase 19.

</domain>

<decisions>
## Implementation Decisions

### API surface change
- **D-01:** Add `max_tier: Option<Tier>` parameter to `select_candidates()`. When `None`, no tier filtering (backward compatible).
- **D-02:** Add `max_tier: Option<Tier>` parameter to `select()` (the convenience wrapper that calls `select_candidates`).
- **D-03:** All existing callers pass `None` for `max_tier` to maintain backward compatibility. Handler integration in Phase 19 will pass actual tier values.

### Tier selection logic
- **D-04:** New free function `score_to_max_tier(score: f64, low: f64, high: f64) -> Tier` in `src/router/complexity.rs`.
- **D-05:** Mapping: score < low → `Tier::Local`, low <= score <= high → `Tier::Standard`, score > high → `Tier::Frontier`.
- **D-06:** Thresholds come from `RoutingConfig.complexity_threshold_low` and `complexity_threshold_high`. The caller reads these from config and passes them to `score_to_max_tier`.

### Filtering mechanics
- **D-07:** Tier filter applied in `select_candidates` as an additional filter predicate: `provider.tier <= max_tier`. Applied AFTER model filtering, BEFORE policy filtering and cost sorting.
- **D-08:** When `max_tier` is set and no providers match the tier + model filter, return `NoPolicyMatch` error (same as when no providers match model/policy). Handler handles escalation in Phase 19.
- **D-09:** When `max_tier` is `None`, the tier filter is skipped entirely (no performance overhead for unscored requests).

### Claude's Discretion
- Exact ordering of filter predicates in select_candidates (model → tier → policy vs model → policy → tier)
- Whether to add tier-specific error variant to Error enum or reuse NoPolicyMatch
- Test structure for the new filtering logic

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Router (primary modification target)
- `src/router/selector.rs` -- `select_candidates()` method (line ~89), `select()` wrapper (line ~68), `SelectedProvider` with `tier` field
- `src/router/complexity.rs` -- `score_complexity()` function, where `score_to_max_tier` will be added

### Config types
- `src/config.rs` lines 182-211 -- `RoutingConfig` with `complexity_threshold_low` (0.4) and `complexity_threshold_high` (0.7)
- `src/config.rs` lines 149-180 -- `Tier` enum with `Ord` derive (`Local < Standard < Frontier`)

### Existing callers of select/select_candidates
- `src/proxy/handlers.rs` line ~248 -- `select_candidates` call in chat_completions handler
- Tests in `src/router/selector.rs` -- all existing unit tests calling `select_candidates`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `Tier` enum already implements `Ord`, `PartialOrd` -- `provider.tier <= max_tier` works directly
- `ProviderConfig` already has `tier: Tier` field from Phase 16
- `SelectedProvider` already carries `tier: Tier` from Phase 16

### Established Patterns
- `select_candidates` filters providers in a chain: model match → policy constraints → cost sort → dedup
- Tier filter fits naturally as another filter predicate in the chain
- Router has no access to config directly -- it stores providers/policies at construction time

### Integration Points
- `select_candidates` is called from handlers.rs (the main caller)
- `select` is a thin wrapper over `select_candidates` used in some tests
- All test `ProviderConfig` literals already include `tier: Tier::Standard` from Phase 16

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

*Phase: 18-tier-aware-routing*
*Context gathered: 2026-04-08*
