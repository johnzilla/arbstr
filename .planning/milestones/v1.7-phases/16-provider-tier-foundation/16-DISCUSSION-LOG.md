# Phase 16: Provider Tier Foundation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md -- this log preserves the alternatives considered.

**Date:** 2026-04-08
**Phase:** 16-provider-tier-foundation
**Areas discussed:** Tier type design, Config surface, Propagation scope

---

## Tier type design

### Ordering semantics

| Option | Description | Selected |
|--------|-------------|----------|
| Local < Standard < Frontier | Natural ordering: local is lowest tier, frontier is highest. `tier <= Standard` matches local + standard. Matches the escalation direction. | ✓ |
| Frontier < Standard < Local | Inverted: frontier is most restrictive, local is most permissive. Less intuitive but some routing systems use this. | |
| No ordering, use sets | Tier has no Ord impl. Router uses explicit tier sets like `vec![Local, Standard]` instead of comparisons. | |

**User's choice:** Local < Standard < Frontier
**Notes:** Matches natural escalation direction (local -> standard -> frontier)

### Serialization

| Option | Description | Selected |
|--------|-------------|----------|
| Lowercase: local, standard, frontier | Matches spec. Simple, readable. `tier = "local"` in TOML, `tier TEXT` column stores same strings. | ✓ |
| Case-insensitive parsing | Accept `Local`, `LOCAL`, `local` in config. Normalize to lowercase for storage/display. More forgiving but adds complexity. | |
| You decide | Claude picks the cleanest approach for the codebase | |

**User's choice:** Lowercase: local, standard, frontier
**Notes:** None

---

## Config surface

| Option | Description | Selected |
|--------|-------------|----------|
| Tier field only | Just add `tier` to [[providers]]. The [routing] section with thresholds comes in Phase 18. Keeps this phase minimal. | |
| Add [routing] skeleton too | Add `tier` on providers AND a `[routing]` config section with threshold fields (parsed but unused until Phase 18). Front-loads config parsing. | ✓ |
| You decide | Claude picks based on what keeps the phase cleanest | |

**User's choice:** Add [routing] skeleton too
**Notes:** Front-load config parsing so Phase 17 and 18 can focus on logic

---

## Propagation scope

| Option | Description | Selected |
|--------|-------------|----------|
| Full propagation | Add tier to SelectedProvider, expose in /providers endpoint response, show in /health per-provider status. Makes tier visible everywhere from day one. | ✓ |
| SelectedProvider only | Carry tier through routing pipeline but don't change endpoint responses yet. Observability endpoints updated in Phase 20. | |
| You decide | Claude picks the pragmatic scope | |

**User's choice:** Full propagation
**Notes:** Tier visible in /providers and /health from day one

## Claude's Discretion

- Exact placement of Tier type (new file vs inline in config.rs)
- Serde attribute strategy for lowercase serialization
- RoutingConfig struct field naming

## Deferred Ideas

None
