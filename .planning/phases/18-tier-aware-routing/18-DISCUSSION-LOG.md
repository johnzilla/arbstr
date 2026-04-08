# Phase 18: Tier-Aware Routing - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md -- this log preserves the alternatives considered.

**Date:** 2026-04-08
**Phase:** 18-tier-aware-routing
**Areas discussed:** API surface change, Tier selection logic, Filtering mechanics

---

## API surface change

| Option | Description | Selected |
|--------|-------------|----------|
| Add max_tier: Option<Tier> param | select_candidates(model, policy_name, prompt, max_tier). None = no filtering. Caller maps score to tier. | ✓ |
| Add score: Option<f64> param | Router internally maps score to tier using thresholds from config. | |
| You decide | Claude picks cleanest approach | |

**User's choice:** Add max_tier: Option<Tier> param
**Notes:** None

---

## Tier selection logic

| Option | Description | Selected |
|--------|-------------|----------|
| Free function in complexity.rs | score_to_max_tier(score, low, high) -> Tier. Keeps scoring logic together. | ✓ |
| Method on RoutingConfig | routing_config.max_tier_for_score(score) -> Tier. Config owns thresholds. | |
| You decide | Claude picks based on codebase patterns | |

**User's choice:** Free function in complexity.rs
**Notes:** None

---

## Filtering mechanics

| Option | Description | Selected |
|--------|-------------|----------|
| Return NoPolicyMatch error | Same error as model/policy mismatch. Handler (Phase 19) handles escalation. Clean separation. | ✓ |
| Fall through to all tiers | Silently include all tiers. Automatic but hides info. | |
| You decide | Claude picks for Phase 19 compat | |

**User's choice:** Return NoPolicyMatch error
**Notes:** Handler escalation is Phase 19's responsibility

## Claude's Discretion

- Filter predicate ordering
- Error variant choice
- Test structure

## Deferred Ideas

None
