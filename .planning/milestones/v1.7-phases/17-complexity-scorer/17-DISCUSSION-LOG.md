# Phase 17: Complexity Scorer - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md -- this log preserves the alternatives considered.

**Date:** 2026-04-08
**Phase:** 17-complexity-scorer
**Areas discussed:** Signal calibration, Scoring formula, Module placement

---

## Signal calibration

### Context length mapping

| Option | Description | Selected |
|--------|-------------|----------|
| Linear with cap | Linear 0.0-1.0 from 0 to ~16K chars. Cap at 1.0. Simple, predictable. | |
| Sigmoid curve | Smooth S-curve centered at ~4K chars. Low scores stay low, high scores saturate. More natural transition. | ✓ |
| Step function | Discrete thresholds: <1K=0.0, 1-4K=0.3, 4-8K=0.6, 8K+=1.0. Predictable, debuggable. | |
| You decide | Claude picks approach for best distribution | |

**User's choice:** Sigmoid curve
**Notes:** None

### Reasoning keywords configurability

| Option | Description | Selected |
|--------|-------------|----------|
| Hardcoded defaults + extra_keywords config | Ship with good defaults. Users can ADD via config but can't remove defaults. Prevents misconfiguration. | ✓ |
| Fully configurable | All keywords in config.toml. Full control but can break scoring. | |
| Hardcoded only | Keywords baked into binary. No config surface. Simplest. | |

**User's choice:** Hardcoded defaults + extra_keywords config
**Notes:** None

---

## Scoring formula

### Combination method

| Option | Description | Selected |
|--------|-------------|----------|
| Weighted average, clamp to 1.0 | Sum(signal_i * weight_i) / Sum(weight_i), clamped to [0.0, 1.0]. Balanced. | ✓ |
| Weighted sum, normalized | Similar but normalization relative to max possible score. | |
| Max-of-signals | Highest weighted sub-score wins. More aggressive escalation. | |

**User's choice:** Weighted average, clamp to 1.0
**Notes:** None

### Default-to-frontier behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Empty/minimal input = 1.0 | If < 10 chars or empty messages, return 1.0. Conservative. | ✓ |
| Any ambiguity = 0.7 | Return 0.7 for unclassifiable. Routes to standard+frontier. | |
| You decide | Claude picks safest default | |

**User's choice:** Empty/minimal input = 1.0
**Notes:** None

---

## Module placement

| Option | Description | Selected |
|--------|-------------|----------|
| src/router/complexity.rs | New file in router module. Matches spec. Clean separation. | ✓ |
| Inline in selector.rs | Fewer files but mixes concerns. | |
| You decide | Claude picks based on codebase conventions | |

**User's choice:** src/router/complexity.rs
**Notes:** None

## Claude's Discretion

- Exact sigmoid parameters
- Individual signal normalization
- Debug struct for sub-scores
- Full reasoning keyword list

## Deferred Ideas

None
