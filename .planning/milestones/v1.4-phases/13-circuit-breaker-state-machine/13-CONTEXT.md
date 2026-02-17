# Phase 13: Circuit Breaker State Machine - Context

**Gathered:** 2026-02-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Per-provider 3-state circuit breaker (Closed, Open, Half-Open) that tracks consecutive failures and recovers automatically. Each provider has its own independent circuit breaker. This phase builds the state machine and its API surface — routing integration (Phase 14) and health endpoint (Phase 15) consume it.

</domain>

<decisions>
## Implementation Decisions

### Failure classification
- Only HTTP 5xx responses and request timeouts count as circuit-tripping failures
- All 4xx responses (including 429 rate limits) are ignored by the circuit breaker
- Network-level errors (connection refused, DNS failure, TLS errors) do NOT trip the circuit — only timeout and 5xx
- Single request timeout model (one duration for the entire request, not separate connect/response timeouts)
- Streaming: only the initial HTTP response status matters — if 2xx is received and streaming begins, it counts as success even if the stream fails mid-way

### Transition logging
- Log state transitions using tracing (not individual failure increments)
- WARN level when circuit opens (something is wrong)
- INFO level when circuit closes or enters half-open (recovery)
- Include reason in log messages: failure count, last error type, provider name (e.g., "provider-alpha circuit OPENED: 3 consecutive 5xx")

### Half-open behavior
- Circuit breaker returns a typed CircuitOpen error when requests hit an open circuit — callers decide what to do
- Single-permit half-open: exactly one probe request allowed
- Queue-and-wait during probe: if probe is in-flight, subsequent requests for that provider wait for the probe result
- All waiting requests wait (no queue limit)
- If probe succeeds: circuit closes, waiting requests proceed
- If probe fails: circuit reopens with fresh 30s timer, all waiting requests receive CircuitOpen error immediately

### Error context
- Store the last error that caused the most recent state change (error type/message)
- Track timestamps for state transitions: opened_at, last failure time, last success time
- Track cumulative trip count (total times this circuit has tripped) — signals chronically unhealthy providers
- No manual reset — circuit breaker is purely automatic

### Claude's Discretion
- Internal data structure layout within the Mutex guard
- Exact CircuitOpen error type design
- Queue-and-wait implementation mechanism (tokio::watch, Notify, etc.)
- Test structure and mock patterns

</decisions>

<specifics>
## Specific Ideas

No specific requirements — open to standard approaches

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 13-circuit-breaker-state-machine*
*Context gathered: 2026-02-16*
