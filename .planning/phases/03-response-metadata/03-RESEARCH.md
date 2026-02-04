# Phase 3: Response Metadata - Research

**Researched:** 2026-02-03
**Domain:** HTTP response header injection in a Rust/axum proxy with per-request metadata (cost, latency, provider, correlation ID)
**Confidence:** HIGH

## Summary

Phase 3 exposes per-request metadata to clients via HTTP response headers. The codebase already computes all required data (cost, latency, provider name, correlation ID) in the `chat_completions` handler as part of Phase 2. The implementation challenge is inserting headers into three distinct response paths: (1) successful non-streaming responses, (2) successful streaming responses, and (3) error responses that go through `Error::into_response()`.

The current handler returns `Result<Response, Error>` from `chat_completions`. Success responses are built inline with `Response::builder()` in `handle_non_streaming_response` and `handle_streaming_response`. Error responses are converted via the `Error::into_response()` trait implementation in `error.rs`, which creates a fresh `Response` with no custom headers. The handler already has `x-arbstr-provider` headers on success responses (added in Phase 2), but error responses have no arbstr headers at all.

The recommended approach is **inline header insertion in the handler** rather than middleware. The handler already has all metadata available (request ID, latency, cost, provider) at the point where it converts `Result<RequestOutcome, RequestError>` into the final HTTP response. Rather than threading metadata through response extensions to a middleware layer, the handler should attach headers directly: to success responses via the existing `Response::builder()` calls, and to error responses by constructing a `Response` with headers instead of returning `Err(Error)`.

**Primary recommendation:** Modify the `chat_completions` handler to build the final HTTP response (with all arbstr headers) for both success and error cases, rather than returning `Err(Error)`. Define header name constants in handlers.rs. Restructure error handling so the handler always returns `Ok(Response)` with appropriate headers, or use a wrapper that attaches headers before converting errors.

## Standard Stack

### Core (no new dependencies)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `axum` | 0.7.9 | `Response::builder()` with `.header()`, `IntoResponse` trait | Already the HTTP framework; response builder supports arbitrary headers |
| `http` | 1.4.0 | `HeaderName`, `HeaderValue`, `StatusCode` | Underlying types for axum responses, already a transitive dependency |
| `uuid` | 1.x | `RequestId.0.to_string()` for `X-Arbstr-Request-Id` header value | Already in deps, used for correlation ID |

### Supporting (no changes needed)

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `std::time::Instant` | stable | Latency measurement (already in handler) | `start.elapsed().as_millis()` for `X-Arbstr-Latency-Ms` |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Inline header insertion in handler | `axum::middleware::from_fn` response middleware | Middleware applies uniformly to all responses (success + error), but requires threading metadata through response extensions. This adds complexity (new extension types, `Extension<ResponseMetadata>` inserts/extracts) for a single endpoint. Inline is simpler and more explicit. |
| Inline header insertion in handler | `axum::middleware::map_response` | Same issue -- lacks access to handler-computed metadata (cost, provider) without response extensions. Also, `map_response` runs for ALL routes, but metadata headers only apply to `/v1/chat/completions`. |
| Restructured handler always returning `Ok(Response)` | `tower_http::set_header::SetResponseHeaderLayer` | Static header layer can't set dynamic per-request values (cost, latency). Only useful for constant headers, which none of these are. |

**No new dependencies needed.** All existing crate versions and features are sufficient.

## Architecture Patterns

### Recommended Approach: Inline Header Insertion

The handler already has a clear separation between outcome computation and response construction. The current flow:

```
chat_completions()
  -> execute_request() -> Ok(RequestOutcome) or Err(RequestError)
  -> log outcome to DB
  -> match result { Ok => Ok(outcome.response), Err => Err(error) }
```

Phase 3 changes the final conversion step to attach headers before returning. Both success and error paths should produce a `Response` with arbstr headers.

### Pattern 1: Header Name Constants

**What:** Define all arbstr response header names as constants alongside the existing `ARBSTR_POLICY_HEADER` request header constant.
**When to use:** Every place that sets or reads arbstr headers.

```rust
// Source: Existing pattern in handlers.rs line 18
/// Custom header for policy selection (request header).
pub const ARBSTR_POLICY_HEADER: &str = "x-arbstr-policy";

// Response headers
pub const ARBSTR_REQUEST_ID_HEADER: &str = "x-arbstr-request-id";
pub const ARBSTR_COST_SATS_HEADER: &str = "x-arbstr-cost-sats";
pub const ARBSTR_LATENCY_MS_HEADER: &str = "x-arbstr-latency-ms";
pub const ARBSTR_PROVIDER_HEADER: &str = "x-arbstr-provider";
pub const ARBSTR_STREAMING_HEADER: &str = "x-arbstr-streaming";
```

### Pattern 2: Non-Streaming Success Response with Full Headers

**What:** Add all four metadata headers to the `Response::builder()` chain in `handle_non_streaming_response`.
**When to use:** Successful non-streaming responses.

The current code already uses `Response::builder().header("x-arbstr-provider", ...)`. Phase 3 extends this with additional headers, but the cost/latency/request-id values are not available inside `handle_non_streaming_response` -- they are computed in the caller (`chat_completions`). Two sub-approaches:

**Sub-approach A (recommended):** Pass metadata values into `handle_non_streaming_response` as parameters, or return the `RequestOutcome` and build the final `Response` with headers in `chat_completions`.

**Sub-approach B:** Return `RequestOutcome` from `handle_non_streaming_response` (as it does now), then build the final `Response` in `chat_completions` by adding headers to the outcome's response.

Sub-approach B is cleaner because the current code already returns `RequestOutcome` containing the `Response`. The caller can add headers to the response using `response.headers_mut()`:

```rust
// Source: http::Response::headers_mut() - axum uses http crate's Response type
let mut response = outcome.response;
response.headers_mut().insert(
    HeaderName::from_static("x-arbstr-request-id"),
    HeaderValue::from_str(&correlation_id).unwrap(),
);
response.headers_mut().insert(
    HeaderName::from_static("x-arbstr-latency-ms"),
    HeaderValue::from(latency_ms as u64),
);
if let Some(cost) = outcome.cost_sats {
    response.headers_mut().insert(
        HeaderName::from_static("x-arbstr-cost-sats"),
        HeaderValue::from_str(&format!("{:.2}", cost)).unwrap(),
    );
}
```

### Pattern 3: Streaming Success Response with Partial Headers

**What:** Streaming responses include `X-Arbstr-Request-Id`, `X-Arbstr-Provider`, and `X-Arbstr-Streaming: true`. They omit `X-Arbstr-Cost-Sats` and `X-Arbstr-Latency-Ms` (not known until stream ends).
**When to use:** Successful streaming responses.

```rust
// Already in handle_streaming_response, extend the builder:
let http_response = Response::builder()
    .status(StatusCode::OK)
    .header(header::CONTENT_TYPE, "text/event-stream")
    .header(header::CACHE_CONTROL, "no-cache")
    .header(ARBSTR_PROVIDER_HEADER, &provider_name)
    .header(ARBSTR_STREAMING_HEADER, "true")
    // Request ID added by caller after return
    .body(body)
    .unwrap();
```

The `X-Arbstr-Request-Id` is added by the caller (Pattern 2 approach), since it is available at that level.

### Pattern 4: Error Response with Headers

**What:** Error responses must include `X-Arbstr-Request-Id` (always), `X-Arbstr-Latency-Ms` (always), `X-Arbstr-Provider` (if known), and `X-Arbstr-Cost-Sats` (if known).
**When to use:** Any error returned from `chat_completions`.

The current code returns `Err(outcome_err.error)` which triggers `Error::into_response()`. This creates a `Response` with no custom headers. There are two approaches:

**Approach A (recommended): Build error response inline in the handler.**

Instead of returning `Err(Error)`, construct the error response manually with headers:

```rust
Err(outcome_err) => {
    let error_response = outcome_err.error.into_response();
    let (mut parts, body) = error_response.into_parts();
    parts.headers.insert(
        HeaderName::from_static("x-arbstr-request-id"),
        HeaderValue::from_str(&correlation_id).unwrap(),
    );
    parts.headers.insert(
        HeaderName::from_static("x-arbstr-latency-ms"),
        HeaderValue::from(latency_ms as u64),
    );
    if let Some(provider) = &outcome_err.provider_name {
        parts.headers.insert(
            HeaderName::from_static("x-arbstr-provider"),
            HeaderValue::from_str(provider).unwrap(),
        );
    }
    Ok(Response::from_parts(parts, body))
}
```

This changes the handler return from `Result<Response, Error>` to always returning `Ok(Response)`, with errors encoded as non-2xx status responses that still carry arbstr headers.

**Approach B: Modify `Error::into_response()` to accept metadata.**

This is not practical because `IntoResponse::into_response(self)` takes only `self` -- there is no way to pass additional context. The trait signature is fixed.

**Approach C: Wrap error in a struct that implements `IntoResponse`.**

Create a type like `ErrorWithHeaders { error: Error, headers: HeaderMap }` that implements `IntoResponse`. This works but adds unnecessary indirection. Approach A is more direct.

### Pattern 5: Helper Function for Header Attachment

**What:** A reusable function that attaches common arbstr headers to any `Response`.
**When to use:** Called in `chat_completions` for both success and error paths to avoid code duplication.

```rust
/// Attach arbstr metadata headers to a response.
fn attach_arbstr_headers(
    response: &mut Response,
    request_id: &str,
    latency_ms: i64,
    provider: Option<&str>,
    cost_sats: Option<f64>,
    is_streaming: bool,
) {
    let headers = response.headers_mut();
    headers.insert(
        HeaderName::from_static("x-arbstr-request-id"),
        HeaderValue::from_str(request_id).unwrap(),
    );
    headers.insert(
        HeaderName::from_static("x-arbstr-latency-ms"),
        HeaderValue::from(latency_ms as u64),
    );
    if let Some(provider) = provider {
        headers.insert(
            HeaderName::from_static("x-arbstr-provider"),
            HeaderValue::from_str(provider).unwrap(),
        );
    }
    if !is_streaming {
        if let Some(cost) = cost_sats {
            headers.insert(
                HeaderName::from_static("x-arbstr-cost-sats"),
                HeaderValue::from_str(&format!("{:.2}", cost)).unwrap(),
            );
        }
    }
    if is_streaming {
        headers.insert(
            HeaderName::from_static("x-arbstr-streaming"),
            HeaderValue::from_static("true"),
        );
    }
}
```

### Pattern 6: Handler Return Type Change

**What:** The handler signature changes from returning `Result<Response, Error>` to returning `Response` (or `Result<Response, Infallible>`).
**When to use:** The `chat_completions` function.

Currently:
```rust
pub async fn chat_completions(...) -> Result<Response, Error> {
```

After Phase 3, since both success and error paths build a full `Response` with headers:
```rust
pub async fn chat_completions(...) -> Response {
```

Or, to keep the `Result` type for consistency with axum patterns but always return `Ok`:
```rust
pub async fn chat_completions(...) -> Result<Response, Error> {
    // ... always returns Ok(response_with_headers)
}
```

The latter is recommended to minimize signature changes and maintain compatibility with the router setup. Pre-execute errors (before any metadata is available, like JSON parse failures from axum's `Json` extractor) will still return through axum's built-in error handling and won't have arbstr headers -- this is acceptable because those errors occur before arbstr processing begins.

### Recommended Project Structure

No new files needed. All changes are within existing files:

```
src/
├── proxy/
│   ├── handlers.rs   # Add header constants, attach_arbstr_headers helper,
│   │                  # modify chat_completions to attach headers on all paths
│   └── server.rs     # No changes needed
├── error.rs          # No changes needed (IntoResponse impl unchanged)
```

### Anti-Patterns to Avoid

- **Middleware for per-endpoint dynamic headers:** Using `from_fn` middleware to inject arbstr headers requires threading metadata through response extensions. This adds complexity (new types, insert/extract ceremony) and runs on all routes, requiring route-path checks. Inline insertion in the handler is simpler and more explicit.
- **Modifying `Error::into_response()` to include headers:** The `IntoResponse` trait takes only `self`. There is no mechanism to pass request-scoped data (correlation ID, latency) into the trait method. Attempting to store metadata in the `Error` enum itself pollutes the error type with presentation concerns.
- **Duplicating header-setting code across success/error paths:** Use a helper function (`attach_arbstr_headers`) to centralize header attachment.
- **Using `HeaderValue::from_str` for integer values:** Use `HeaderValue::from(u64)` for integer header values (latency). It avoids string allocation and is infallible.
- **Setting `X-Arbstr-Cost-Sats` on streaming responses:** Per CONTEXT.md decision, streaming responses omit cost and latency headers because these values are not known when headers are sent.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Header name/value types | Manual string-to-header conversion | `HeaderName::from_static()` for compile-time-known names, `HeaderValue::from()` for integers | `from_static` is zero-cost for `&'static str`, `from()` for integers is infallible |
| Response decomposition for header injection | Manual body copying | `response.into_parts()` + `Response::from_parts()` | Standard http crate pattern for modifying a response's headers without touching the body |
| Cost formatting with precision | Manual float formatting | `format!("{:.2}", cost)` | Produces consistent decimal representation like "42.35" |

**Key insight:** The `http::Response` type (used by axum) has `headers_mut()` for in-place header modification and `into_parts()`/`from_parts()` for decomposing and rebuilding. Both are zero-copy for the body. There is no need for wrapper types or middleware to add headers.

## Common Pitfalls

### Pitfall 1: Error Responses Missing Arbstr Headers

**What goes wrong:** The handler returns `Err(Error)` which triggers `Error::into_response()`. This method creates a fresh `Response` with no arbstr headers. Clients see error responses without `X-Arbstr-Request-Id`, making it impossible to correlate errors with logs.
**Why it happens:** The `IntoResponse` trait signature does not accept additional context. The error type does not carry request-scoped metadata.
**How to avoid:** Never return `Err(Error)` from `chat_completions` after Phase 3. Instead, convert the error into a `Response` using `error.into_response()`, then add headers to it using `headers_mut()`, and return `Ok(response)`.
**Warning signs:** Error responses from `/v1/chat/completions` missing `X-Arbstr-Request-Id` header.

### Pitfall 2: Pre-execute Errors Lacking Headers

**What goes wrong:** Errors that occur before `chat_completions` runs (e.g., JSON deserialization failure from axum's `Json` extractor, or a missing Content-Type) bypass the handler entirely. These responses will not have arbstr headers.
**Why it happens:** Axum's built-in extractors return their own error responses before the handler function body executes. The handler never runs, so it cannot add headers.
**How to avoid:** Accept this limitation. Pre-handler errors (malformed JSON, wrong content type) are client errors unrelated to arbstr's routing. They won't have a correlation ID in the logs either, because the handler never generated one. Document this as a known boundary.
**Warning signs:** `400 Bad Request` responses from malformed JSON lacking `X-Arbstr-Request-Id`. This is expected and acceptable.

### Pitfall 3: HeaderValue Parse Failures

**What goes wrong:** `HeaderValue::from_str()` returns `Err` if the string contains invalid header value characters (control characters, non-visible ASCII). If provider names contain such characters, header insertion silently fails or panics on `.unwrap()`.
**Why it happens:** HTTP header values have restrictions (RFC 7230: visible ASCII plus spaces and tabs). Provider names from config could theoretically contain non-ASCII.
**How to avoid:** Provider names are defined in the config file and validated at startup (or should be). For defense-in-depth, use `.unwrap_or_else(|_| HeaderValue::from_static("unknown"))` or simply `.unwrap()` since config-sourced names are under user control. UUID strings are always valid header values.
**Warning signs:** Panic on `HeaderValue::from_str().unwrap()` with unusual provider names.

### Pitfall 4: Latency Timer Placement

**What goes wrong:** Starting the latency timer too early (before request parsing) or too late (after provider selection) gives misleading latency values.
**Why it happens:** The timer should capture the full proxy overhead including routing, not just the upstream call.
**How to avoid:** The timer already starts at the first line of `chat_completions` (`let start = std::time::Instant::now()`). This is the correct placement -- it measures from when axum dispatches to the handler to when the response is ready. It does NOT include axum's own request parsing time, which is negligible.
**Warning signs:** Latency values that seem too low (missing routing time) or too high (including TCP accept time).

### Pitfall 5: Streaming Latency Semantics

**What goes wrong:** For streaming responses, `latency_ms` is computed when `chat_completions` returns the `Response` containing the stream body. At this point, the stream has NOT been consumed. The latency reflects time-to-first-byte (time to set up the stream), not time-to-completion.
**Why it happens:** The `Response` with its streaming body is returned immediately; the SSE chunks are consumed by the client asynchronously.
**How to avoid:** Accept this as the correct semantic for streaming latency in the header. The `X-Arbstr-Latency-Ms` on a streaming response measures proxy setup time (routing + upstream connection + first response headers). This is the only value available when HTTP response headers are sent. Per CONTEXT.md, streaming responses omit the latency header entirely, so this pitfall is moot for Phase 3 -- but the database log still captures this value.
**Warning signs:** None -- streaming responses omit `X-Arbstr-Latency-Ms` per design.

### Pitfall 6: Duplicate X-Arbstr-Provider Header

**What goes wrong:** The existing code already sets `x-arbstr-provider` in `handle_non_streaming_response` and `handle_streaming_response`. If the helper function also sets it, the header appears twice.
**Why it happens:** Phase 2 already added `x-arbstr-provider` to the response builder in both response handlers.
**How to avoid:** Two options: (a) Remove the header from the response builders in `handle_*_response` and let the helper function add it, or (b) use `headers.insert()` (which replaces existing values) rather than `headers.append()` (which adds duplicates). `insert()` is the correct choice since we want exactly one value per header.
**Warning signs:** `curl -v` showing `x-arbstr-provider` appearing twice in response headers.

## Code Examples

### Example 1: Header Constants

```rust
// Source: Existing pattern at handlers.rs:18, extended for response headers

/// Custom header for policy selection (request header).
pub const ARBSTR_POLICY_HEADER: &str = "x-arbstr-policy";

/// Response header: correlation ID (UUID v4).
pub const ARBSTR_REQUEST_ID_HEADER: &str = "x-arbstr-request-id";
/// Response header: actual cost in satoshis (decimal, e.g. "42.35").
pub const ARBSTR_COST_SATS_HEADER: &str = "x-arbstr-cost-sats";
/// Response header: wall-clock latency in milliseconds (integer).
pub const ARBSTR_LATENCY_MS_HEADER: &str = "x-arbstr-latency-ms";
/// Response header: provider name that handled the request.
pub const ARBSTR_PROVIDER_HEADER: &str = "x-arbstr-provider";
/// Response header: present with value "true" on streaming responses.
pub const ARBSTR_STREAMING_HEADER: &str = "x-arbstr-streaming";
```

### Example 2: Helper Function for Header Attachment

```rust
// Source: http::Response::headers_mut(), HeaderName::from_static(), HeaderValue::from()

use axum::http::{HeaderName, HeaderValue};

/// Attach arbstr metadata headers to a response.
///
/// For non-streaming responses: sets request-id, latency, provider, and cost.
/// For streaming responses: sets request-id, provider, and streaming flag.
/// Cost is omitted if not known (None) or if streaming.
fn attach_arbstr_headers(
    response: &mut Response,
    request_id: &str,
    latency_ms: i64,
    provider: Option<&str>,
    cost_sats: Option<f64>,
    is_streaming: bool,
) {
    let headers = response.headers_mut();

    // Always present
    headers.insert(
        HeaderName::from_static(ARBSTR_REQUEST_ID_HEADER),
        HeaderValue::from_str(request_id).unwrap(),
    );

    if is_streaming {
        headers.insert(
            HeaderName::from_static(ARBSTR_STREAMING_HEADER),
            HeaderValue::from_static("true"),
        );
        // Streaming: omit cost and latency (not known at header-send time)
    } else {
        // Non-streaming: always include latency
        headers.insert(
            HeaderName::from_static(ARBSTR_LATENCY_MS_HEADER),
            HeaderValue::from(latency_ms as u64),
        );
        // Non-streaming: include cost if known
        if let Some(cost) = cost_sats {
            headers.insert(
                HeaderName::from_static(ARBSTR_COST_SATS_HEADER),
                HeaderValue::from_str(&format!("{:.2}", cost)).unwrap(),
            );
        }
    }

    // Provider: present when known
    if let Some(provider_name) = provider {
        headers.insert(
            HeaderName::from_static(ARBSTR_PROVIDER_HEADER),
            HeaderValue::from_str(provider_name).unwrap(),
        );
    }
}
```

### Example 3: Error Response with Headers (Handler Pattern)

```rust
// Source: http::Response::into_parts/from_parts, Error::into_response()

// In chat_completions, the error path changes from:
//   Err(outcome_err) => Err(outcome_err.error),
// to:
Err(outcome_err) => {
    let mut error_response = outcome_err.error.into_response();
    attach_arbstr_headers(
        &mut error_response,
        &correlation_id,
        latency_ms,
        outcome_err.provider_name.as_deref(),
        None,  // cost not known for errors (unless partial)
        is_streaming,
    );
    Ok(error_response)
}
```

### Example 4: Success Response with Headers (Handler Pattern)

```rust
// In chat_completions, the success path changes from:
//   Ok(outcome) => Ok(outcome.response),
// to:
Ok(outcome) => {
    let mut response = outcome.response;
    attach_arbstr_headers(
        &mut response,
        &correlation_id,
        latency_ms,
        Some(&outcome.provider_name),
        outcome.cost_sats,
        is_streaming,
    );
    Ok(response)
}
```

### Example 5: Remove Duplicate Provider Header from Response Builders

```rust
// In handle_non_streaming_response, REMOVE the existing x-arbstr-provider header:
// Before:
let http_response = Response::builder()
    .status(StatusCode::OK)
    .header(header::CONTENT_TYPE, "application/json")
    .header("x-arbstr-provider", &provider.name)  // REMOVE THIS
    .body(Body::from(serde_json::to_vec(&response).unwrap()))
    .unwrap();

// After:
let http_response = Response::builder()
    .status(StatusCode::OK)
    .header(header::CONTENT_TYPE, "application/json")
    .body(Body::from(serde_json::to_vec(&response).unwrap()))
    .unwrap();

// Similarly in handle_streaming_response, remove the x-arbstr-provider line.
// The helper function in chat_completions will add all arbstr headers uniformly.
```

### Example 6: Verifying Headers with curl

```bash
# Non-streaming response -- expect all headers
curl -s -D - http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}' \
  2>&1 | grep -i 'x-arbstr'

# Expected output:
# x-arbstr-request-id: 550e8400-e29b-41d4-a716-446655440000
# x-arbstr-cost-sats: 42.35
# x-arbstr-latency-ms: 1523
# x-arbstr-provider: provider-alpha

# Streaming response -- expect partial headers
curl -s -D - http://localhost:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}],"stream":true}' \
  2>&1 | grep -i 'x-arbstr'

# Expected output:
# x-arbstr-request-id: 550e8400-e29b-41d4-a716-446655440000
# x-arbstr-provider: provider-alpha
# x-arbstr-streaming: true
```

## State of the Art

| Old Approach (current code) | Current Approach (Phase 3 target) | Impact |
|-----|------|--------|
| `x-arbstr-provider` header set only on success responses | All arbstr headers on both success and error responses | Clients can always correlate responses with logs |
| Error responses have no arbstr metadata | Error responses include request-id, latency, provider | Debugging failures possible with header inspection |
| No cost header | `X-Arbstr-Cost-Sats` on non-streaming successes | Clients see per-request cost without parsing body |
| No latency header | `X-Arbstr-Latency-Ms` on non-streaming responses and errors | Clients see proxy latency without external timing |
| No streaming indicator | `X-Arbstr-Streaming: true` on streaming responses | Clients know which headers to expect |
| Handler returns `Result<Response, Error>` | Handler always returns `Ok(Response)` with headers | Unified response construction path |

**Deprecated/outdated:**
- The inline `x-arbstr-provider` header in `handle_non_streaming_response` (line 280) and `handle_streaming_response` (line 357) should be removed and replaced by the centralized helper function.
- The `arbstr_provider` body field injection (line 270-275) is separate from headers and remains unchanged.

## Claude's Discretion Recommendations

### Header insertion: Inline in handler (not middleware)

**Recommendation:** Use inline header insertion in the `chat_completions` handler via a helper function.

**Rationale:**
1. All required metadata (request ID, latency, cost, provider) is already computed in `chat_completions`.
2. Middleware would require threading metadata via response extensions -- added complexity for no benefit.
3. Only one endpoint (`/v1/chat/completions`) needs these headers. Middleware runs on all routes and would need route filtering.
4. The helper function pattern keeps header logic explicit and testable.
5. Error responses require special handling regardless (converting `Err(Error)` to `Ok(Response)` with headers).

### Latency measurement: Keep current timer placement

**Recommendation:** Keep the existing `let start = std::time::Instant::now()` at the top of `chat_completions`.

**Rationale:**
1. The timer is already in the correct position (first line of handler, measured at line 56).
2. It captures the full proxy overhead: routing, upstream request, response parsing.
3. It does NOT include axum's request parsing overhead, which is negligible and not meaningful.
4. For error responses, it correctly shows time-before-failure (e.g., DNS timeout vs immediate rejection).
5. For streaming, it measures time-to-stream-setup, which is the only value available when headers are sent (but streaming responses omit this header per the decision).

### Header ordering: No specific order required

**Recommendation:** Use the natural order from the helper function (request-id first, then latency, then cost, then provider, then streaming).

**Rationale:**
1. HTTP headers are unordered by spec (RFC 7230 Section 3.2.2 states that multiple headers with different field names can appear in any order).
2. No client depends on header ordering.
3. The helper function produces a consistent order for human readability in `curl -v` output.

## Open Questions

1. **Cost decimal precision**
   - What we know: CONTEXT.md says "Decimal sats with sub-satoshi precision (e.g. `42.35`)". The `actual_cost_sats` function returns `f64`. The example shows 2 decimal places.
   - What's unclear: Whether to always use 2 decimal places (`format!("{:.2}", cost)`) or use full f64 precision. Two decimal places matches the example but could truncate meaningful sub-sat values for very cheap models (e.g., `0.125` becomes `0.12` -- 4% error).
   - Recommendation: Use full precision with trailing zero trimming, or use a fixed precision that captures sub-sat values meaningfully. Since the CONTEXT.md example uses `42.35`, use `{:.2}` (two decimal places) for consistency with the documented format. The 4% precision loss at sub-sat levels is negligible for cost tracking. If this becomes an issue, precision can be increased later without breaking clients (header values are strings).

2. **Error response cost header**
   - What we know: CONTEXT.md says `X-Arbstr-Cost-Sats` is "Present if cost is known (tokens consumed before error), omitted otherwise." In the current code, errors from `execute_request` have no token/cost data (the `RequestError` struct has no cost fields).
   - What's unclear: Whether any error scenario in the current code could have a known cost.
   - Recommendation: For Phase 3, always omit `X-Arbstr-Cost-Sats` on error responses (pass `None` to the helper). No current error path captures partial token usage. This matches the "omitted otherwise" clause. If future phases add partial-failure handling (e.g., error after partial stream consumption), the cost can be added then.

## Sources

### Primary (HIGH confidence)
- Local codebase analysis -- all source files read (`handlers.rs`, `server.rs`, `types.rs`, `error.rs`, `selector.rs`, `logging.rs`, `mod.rs`, `lib.rs`, `Cargo.toml`), current handler structure, error paths, and Phase 2 implementation fully understood
- [axum 0.7 response docs](https://docs.rs/axum/latest/axum/response/index.html) -- `IntoResponse` trait, tuple composition, `HeaderMap` in responses
- [axum 0.7 middleware docs](https://docs.rs/axum/latest/axum/middleware/index.html) -- `from_fn`, `map_response`, layer ordering
- [axum `IntoResponseParts` docs](https://docs.rs/axum/latest/axum/response/trait.IntoResponseParts.html) -- response parts composition
- [http crate `Response` docs](https://docs.rs/http/1/http/response/struct.Response.html) -- `headers_mut()`, `into_parts()`/`from_parts()`, `HeaderName::from_static()`, `HeaderValue::from()`
- `cargo tree` output -- axum 0.7.9, http 1.4.0, axum-core 0.4.5 versions confirmed

### Secondary (MEDIUM confidence)
- [axum discussion #1131 (response headers in middleware)](https://github.com/tokio-rs/axum/discussions/1131) -- Pattern for `from_fn` middleware modifying response headers, confirmed by maintainer responses
- [axum discussion #2953 (deferred response generation)](https://github.com/tokio-rs/axum/discussions/2953) -- Response extensions for handler-to-middleware data passing
- [axum `map_response` docs](https://docs.rs/axum/latest/axum/middleware/fn.map_response.html) -- Confirmed `map_response` runs on all responses including error status codes

### Tertiary (LOW confidence)
- None. All findings verified against codebase or official documentation.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH -- No new dependencies needed, all APIs verified against docs.rs and local `cargo tree`
- Architecture: HIGH -- Inline header insertion pattern derived from codebase analysis (existing `Response::builder().header()` calls), `headers_mut()` verified in http crate docs, error response restructuring pattern verified against `IntoResponse` trait signature
- Pitfalls: HIGH -- Duplicate header issue identified from existing code (lines 280, 357), `Error::into_response()` limitation verified from trait signature, pre-handler error boundary identified from axum extractor behavior

**Research date:** 2026-02-03
**Valid until:** 2026-03-03 (stable domain -- Rust ecosystem, no fast-moving dependencies)
