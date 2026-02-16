# Phase 12: Request Log Listing - Context

**Gathered:** 2026-02-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Paginated browsing of individual request records with filtering and sorting via GET endpoint. Users can investigate specific requests logged by arbstr's SQLite storage. Aggregate analytics, dashboards, and export are separate concerns.

</domain>

<decisions>
## Implementation Decisions

### Response shape
- Curated field set — exclude correlation_id, provider_cost_sats, and policy from response
- Included fields: id, timestamp, model, provider, streaming, input_tokens, output_tokens, cost_sats, latency_ms, stream_duration_ms, success, error_status, error_message
- Nested sections per record: group related fields (timing, costs, tokens) rather than flat objects
- Top-level wrapper includes pagination metadata AND effective time range (since/until)

### Pagination style
- Page-based: `page` and `per_page` query params
- Default page size: 20, maximum: 100
- Response includes both `total` count and `total_pages` convenience field
- Out-of-range pages return 200 with empty data array (not 400)

### Filter behavior
- Multiple filters combine with AND (all must match)
- Reuse same time range params as /v1/stats: `since`, `until`, `range` presets — identical behavior
- Success filter: `success=true` or `success=false` (boolean query param)
- Non-existent model/provider values return 404 (consistent with /v1/stats)
- Streaming filter: `streaming=true` or `streaming=false`

### Sort defaults
- Default: newest first (timestamp descending) when no sort param provided
- Param style: `sort=<field>&order=asc|desc` (two separate params)
- Valid sort fields: timestamp, cost_sats, latency_ms
- Invalid sort field returns 400 with list of valid options
- Single column sort only (no multi-column)

### Claude's Discretion
- Exact nested section names and structure within each record
- How to handle the default time range for logs (whether to default to last_7d like stats or show all)
- Error response format details

</decisions>

<specifics>
## Specific Ideas

- API consistency with /v1/stats is important — same time range params, same 404 behavior for bad filters, same case-insensitive matching
- Response wrapper pattern: `{data: [...], page, per_page, total, total_pages, since, until}`

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 12-request-log-listing*
*Context gathered: 2026-02-16*
