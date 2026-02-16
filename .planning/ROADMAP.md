# Roadmap: arbstr

## Milestones

- SHIPPED **v1 Reliability and Observability** -- Phases 1-4 (shipped 2026-02-04)
- SHIPPED **v1.1 Secrets Hardening** -- Phases 5-7 (shipped 2026-02-15)
- IN PROGRESS **v1.2 Streaming Observability** -- Phases 8-10

## Phases

<details>
<summary>SHIPPED v1 Reliability and Observability (Phases 1-4) -- SHIPPED 2026-02-04</summary>

- [x] Phase 1: Foundation (2/2 plans) -- completed 2026-02-02
- [x] Phase 2: Request Logging (4/4 plans) -- completed 2026-02-04
- [x] Phase 3: Response Metadata (1/1 plan) -- completed 2026-02-04
- [x] Phase 4: Retry and Fallback (3/3 plans) -- completed 2026-02-04

See: .planning/milestones/v1-ROADMAP.md for full details.

</details>

<details>
<summary>SHIPPED v1.1 Secrets Hardening (Phases 5-7) -- SHIPPED 2026-02-15</summary>

- [x] Phase 5: Secret Type Foundation (1/1 plan) -- completed 2026-02-15
- [x] Phase 6: Environment Variable Expansion (2/2 plans) -- completed 2026-02-15
- [x] Phase 7: Output Surface Hardening (1/1 plan) -- completed 2026-02-15

See: .planning/milestones/v1.1-ROADMAP.md for full details.

</details>

### v1.2 Streaming Observability

**Milestone Goal:** Complete the logging story by extracting token counts and costs from streaming responses, ensuring every request has accurate observability data.

- [ ] **Phase 8: Stream Request Foundation** - Inject stream_options into upstream requests and add post-stream database UPDATE capability
- [ ] **Phase 9: SSE Stream Interception** - Line-buffered SSE parser that extracts usage data from streaming responses
- [ ] **Phase 10: Streaming Observability Integration** - Wire stream interception into handlers with full latency tracking, completion status, and trailing cost events

## Phase Details

### Phase 8: Stream Request Foundation
**Goal**: Upstream requests include stream_options so providers send usage data, and the database can accept post-stream token/cost updates
**Depends on**: Nothing (independent of Phase 9)
**Requirements**: STREAM-01, COST-01
**Success Criteria** (what must be TRUE):
  1. When arbstr forwards a streaming request, the upstream payload includes `stream_options: {"include_usage": true}` regardless of what the client sent
  2. Non-streaming requests are unmodified (no stream_options injected)
  3. A database UPDATE function can write token counts and cost to an existing request log entry by correlation_id
  4. Existing tests pass unchanged (backward compatible type additions)
**Plans**: TBD

### Phase 9: SSE Stream Interception
**Goal**: A standalone stream wrapper module can buffer SSE lines across chunk boundaries and extract usage data from the final chunk
**Depends on**: Nothing (independent of Phase 8)
**Requirements**: STREAM-02, EXTRACT-01
**Success Criteria** (what must be TRUE):
  1. SSE data lines split across TCP chunk boundaries are reassembled correctly without data loss
  2. The usage object (prompt_tokens, completion_tokens) is extracted from the final SSE chunk when present
  3. Streams without usage data (unsupported providers) pass through without error, yielding no extracted values
  4. The stream wrapper passes all bytes through unmodified to the client (observation-only, zero content mutation)
**Plans**: TBD

### Phase 10: Streaming Observability Integration
**Goal**: Every streaming request logs accurate token counts, cost, full-duration latency, and completion status, with cost surfaced to clients via trailing SSE event
**Depends on**: Phase 8, Phase 9
**Requirements**: COST-02, OBS-01, OBS-02
**Success Criteria** (what must be TRUE):
  1. After a streaming response completes, the database row for that request contains non-NULL input_tokens, output_tokens, and cost_sats (when provider sends usage)
  2. Streaming request latency_ms reflects time-to-last-byte (full stream duration), not time-to-first-byte
  3. Stream completion status distinguishes normal completion, client disconnection, and provider error in the log
  4. After the upstream `[DONE]` marker, the client receives a trailing SSE event containing `arbstr_cost_sats` and `arbstr_latency_ms`
  5. Providers that do not send usage data degrade gracefully to NULL tokens and cost (no regression from current behavior)
**Plans**: TBD

## Progress

**Execution Order:**
Phase 8 and Phase 9 are independent and can execute in either order. Phase 10 requires both.

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Foundation | v1 | 2/2 | Complete | 2026-02-02 |
| 2. Request Logging | v1 | 4/4 | Complete | 2026-02-04 |
| 3. Response Metadata | v1 | 1/1 | Complete | 2026-02-04 |
| 4. Retry and Fallback | v1 | 3/3 | Complete | 2026-02-04 |
| 5. Secret Type Foundation | v1.1 | 1/1 | Complete | 2026-02-15 |
| 6. Environment Variable Expansion | v1.1 | 2/2 | Complete | 2026-02-15 |
| 7. Output Surface Hardening | v1.1 | 1/1 | Complete | 2026-02-15 |
| 8. Stream Request Foundation | v1.2 | 0/? | Not started | - |
| 9. SSE Stream Interception | v1.2 | 0/? | Not started | - |
| 10. Streaming Observability Integration | v1.2 | 0/? | Not started | - |
