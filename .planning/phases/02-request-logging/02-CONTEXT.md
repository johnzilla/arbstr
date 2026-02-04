# Phase 2: Request Logging - Context

**Gathered:** 2026-02-03
**Status:** Ready for planning

<domain>
## Phase Boundary

Every completed request is persistently logged to SQLite with accurate token counts, costs, and latency. Covers both streaming and non-streaming responses. Includes failed requests and pre-route rejections. Does NOT include query endpoints, dashboards, or learned token ratios (population of token_ratios table is future work).

</domain>

<decisions>
## Implementation Decisions

### Token extraction
- Non-streaming: extract usage object from provider JSON response; if missing or incomplete, log with null token/cost fields
- Streaming: watch chunks as they pass through to the client, capture usage from the final SSE chunk without buffering the full response
- Cost: log both arbstr-calculated cost (via actual_cost_sats with config rates) AND provider-reported cost in separate columns, when available

### Schema design
- Create both `requests` and `token_ratios` tables in initial migration (token_ratios won't be populated yet but avoids a future migration)
- Integer auto-increment primary key; correlation UUID stored as a separate indexed TEXT column (`correlation_id`)
- Additional columns beyond CLAUDE.md baseline: `streaming` (boolean), `provider_cost_sats` (provider-reported cost, nullable)
- Error context: `error_status` (integer, HTTP status code) and `error_message` (text, short description) for failed requests

### Write behavior
- Use sqlx embedded migrations (migrate!() macro with .sql files in migrations/ directory), applied automatically on startup
- DB file auto-created by SQLite on first connection if it doesn't exist — zero user setup
- SqlitePool added to AppState, accessed through axum State extractor
- Async writes via tokio::spawn fire-and-forget after response completes; if write fails, log warning and move on

### Failure logging
- Log ALL requests to the same `requests` table, including provider errors (success=false) and pre-route rejections (no provider contacted)
- Failed requests: store HTTP status code + short error message; null tokens/cost; latency reflects time until failure
- Pre-route rejections (no matching provider, policy violation): log the requested model; provider is null

### Claude's Discretion
- Exact migration SQL and column types
- SqlitePool configuration (pool size, timeouts)
- How to structure the storage module internally
- Error type additions needed for DB operations

</decisions>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches. Follow existing codebase patterns (axum State, thiserror, tracing).

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 02-request-logging*
*Context gathered: 2026-02-03*
