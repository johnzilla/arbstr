# Phase 22: Vault Fault Tolerance - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-09
**Phase:** 22-vault-fault-tolerance
**Areas discussed:** Test strategy, Reconciliation behavior

---

## Test Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Direct DB insertion | Insert pending rows, run reconciliation, verify replay | |
| Full cycle simulation | Fail mid-settle, verify pending written, run reconciliation | |
| Both approaches | Direct insertion for unit tests + full cycle for integration | ✓ |

**User's choice:** Both approaches

---

## Reconciliation Behavior — Stale Settlements

| Option | Description | Selected |
|--------|-------------|----------|
| Log warning, keep retrying | Current behavior — infinite retry | |
| Evict after N attempts | Delete with warning after threshold | ✓ |
| Alert operator, stop retrying | Error log, stop retrying specific settlement | |

**User's choice:** Evict after N attempts

**Follow-up: Max attempts**

| Option | Description | Selected |
|--------|-------------|----------|
| 10 attempts | ~10 minutes at 60s intervals | ✓ |
| 20 attempts | ~20 minutes | |
| You decide | Claude picks | |

**User's choice:** 10 attempts

---

## Claude's Discretion

- Configurable vs hardcoded max_attempts
- Test structure and naming
- Reconciliation pass return value

## Deferred Ideas

None
