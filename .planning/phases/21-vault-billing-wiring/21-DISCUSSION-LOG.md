# Phase 21: Vault Billing Wiring - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-09
**Phase:** 21-vault-billing-wiring
**Areas discussed:** Auth token unification, Reserve pricing strategy, Error responses to client, End-to-end test strategy

---

## Auth Token Unification

| Option | Description | Selected |
|--------|-------------|----------|
| Vault replaces server auth | Skip server-level auth_token middleware when vault is configured | |
| Both layers active | Server auth_token gates access, vault agent token gates billing | ✓ |
| You decide | Claude picks cleanest approach | |

**User's choice:** Both layers active

**Follow-up: Header conflict resolution**

| Option | Description | Selected |
|--------|-------------|----------|
| Server auth moves to X-Arbstr-Token | Custom header for server auth | |
| Server auth stays on Bearer | Vault agent token in separate header | |
| You decide | Claude picks based on OpenAI compatibility | |

**User's choice:** Custom header for both? (free text)

**Resolution:** After discussion, agreed that `Authorization: Bearer` serves both purposes — vault validates when configured, server auth_token checks when vault absent. No custom headers. OpenAI clients just set their API key and it works in both modes.

---

## Reserve Pricing Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Always reserve at frontier rates | Most expensive tier's rates for ceiling, overage refunded | ✓ |
| Reserve at scored tier + buffer | Scored tier rates with 2x safety buffer | |
| Reserve at max available tier | Scan all candidates for highest-tier rates | |

**User's choice:** Always reserve at frontier rates
**Notes:** Simple and safe approach. Overage refunded on settle.

---

## Error Responses to Client

| Option | Description | Selected |
|--------|-------------|----------|
| OpenAI-compatible JSON | Wrap vault errors in standard OpenAI error format | ✓ |
| Pass through vault response | Forward vault's native JSON and status codes | |
| You decide | Claude picks based on existing patterns | |

**User's choice:** OpenAI-compatible JSON
**Notes:** Consistent with existing error.rs patterns.

---

## End-to-End Test Strategy

| Option | Description | Selected |
|--------|-------------|----------|
| Mock HTTP vault in Rust tests | Lightweight HTTP server mimicking vault endpoints | ✓ |
| Use arbstr-vault simulated mode | Run real vault service alongside Rust tests | |
| Both layers | Mock for CI, real vault for manual E2E | |

**User's choice:** Mock HTTP vault in Rust tests
**Notes:** Keeps test suite self-contained with no TypeScript dependency.

---

## Claude's Discretion

- Implementation details of mock HTTP vault server in tests
- Exact OpenAI error type/code strings for vault errors
- Whether to add new error variants to error.rs or reuse existing ones

## Deferred Ideas

None
