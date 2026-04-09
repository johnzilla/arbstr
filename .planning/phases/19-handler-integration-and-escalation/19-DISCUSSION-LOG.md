# Phase 19: Handler Integration and Escalation - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-04-08
**Phase:** 19-handler-integration-and-escalation
**Areas discussed:** Scoring call site, Escalation loop, Header override

---

## Scoring call site

| Option | Description | Selected |
|--------|-------------|----------|
| Inside resolve_candidates | Modify resolve_candidates to score, compute max_tier, pass to select_candidates. Localized. | ✓ |
| Before resolve_candidates | Score in main handler, pass max_tier as parameter. More explicit. | |
| You decide | | |

**User's choice:** Inside resolve_candidates

---

## Escalation loop

| Option | Description | Selected |
|--------|-------------|----------|
| Expand and retry in resolve_candidates | If NoPolicyMatch with tier filter, try next tier up. Max 2 escalations. Contained in one function. | ✓ |
| Caller escalation loop | resolve_candidates returns error, handler retries with escalated tier. | |
| You decide | | |

**User's choice:** Expand and retry in resolve_candidates

---

## Header override

| Option | Description | Selected |
|--------|-------------|----------|
| Case-insensitive, high\|low only | high→Frontier, low→Local. Invalid ignored. | |
| Case-insensitive, high\|low\|medium | Add medium→Standard. More granular. | ✓ |
| You decide | | |

**User's choice:** Case-insensitive, high|low|medium

## Claude's Discretion

- resolve_candidates signature changes
- Error matching for escalation
- NoTierMatch error variant

## Deferred Ideas

None
