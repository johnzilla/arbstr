# Phase 20: Routing Observability - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-04-09
**Phase:** 20-routing-observability
**Areas discussed:** Header + SSE format, DB schema + stats

---

## Header + SSE format

### Score precision

| Option | Description | Selected |
|--------|-------------|----------|
| 3 decimal places | x-arbstr-complexity-score: 0.423. Clean, sufficient. | ✓ |
| Full f64 precision | Maximum detail but verbose. | |
| You decide | | |

**User's choice:** 3 decimal places

---

## DB schema + stats

### Column nullability

| Option | Description | Selected |
|--------|-------------|----------|
| Nullable (recommended) | complexity_score REAL NULL, tier TEXT NULL. Old rows NULL. | ✓ |
| Required with defaults | DEFAULT 0.0 / 'standard'. Simpler queries. | |
| You decide | | |

**User's choice:** Nullable

## Claude's Discretion

- Streaming header behavior
- SQL for tier breakdown
- Tier column indexes
- Filter params for /v1/requests

## Deferred Ideas

None
