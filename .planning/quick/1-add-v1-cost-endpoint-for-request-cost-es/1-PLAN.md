---
phase: quick
plan: 1
type: execute
wave: 1
depends_on: []
files_modified:
  - src/proxy/handlers.rs
  - src/proxy/server.rs
  - tests/cost.rs
autonomous: true
requirements: [COST-01]
must_haves:
  truths:
    - "POST /v1/cost with a valid chat completion body returns 200 with provider, cost estimate, and token estimates"
    - "POST /v1/cost with an unknown model returns 400 (same error as chat_completions routing)"
    - "POST /v1/cost respects X-Arbstr-Policy header for provider selection"
    - "POST /v1/cost requires auth when auth_token is configured (same as /v1/chat/completions)"
    - "Endpoint does NOT call any upstream provider"
  artifacts:
    - path: "src/proxy/handlers.rs"
      provides: "cost_estimate handler function"
      contains: "pub async fn cost_estimate"
    - path: "src/proxy/server.rs"
      provides: "POST /v1/cost route registration"
      contains: "/v1/cost"
    - path: "tests/cost.rs"
      provides: "Integration tests for /v1/cost endpoint"
      min_lines: 80
  key_links:
    - from: "src/proxy/server.rs"
      to: "src/proxy/handlers.rs"
      via: "route registration"
      pattern: "handlers::cost_estimate"
    - from: "src/proxy/handlers.rs"
      to: "src/router/selector.rs"
      via: "router.select() for provider selection"
      pattern: "state\\.router\\.select"
---

<objective>
Add a POST /v1/cost endpoint that accepts the same request body as /v1/chat/completions
but returns a cost estimate instead of proxying to a provider.

Purpose: Let clients preview what a request would cost in sats before committing to it.
Output: Working /v1/cost endpoint with integration tests.
</objective>

<execution_context>
@/home/john/.claude/get-shit-done/workflows/execute-plan.md
@/home/john/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/proxy/handlers.rs
@src/proxy/server.rs
@src/proxy/types.rs
@src/router/selector.rs
@src/config.rs
@src/error.rs
@tests/common/mod.rs
@tests/health.rs

<interfaces>
<!-- Key types and contracts the executor needs. -->

From src/proxy/types.rs:
```rust
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: Option<u32>,
    // ... other fields
}
impl ChatCompletionRequest {
    pub fn user_prompt(&self) -> Option<&str>;
}
pub enum MessageContent { Text(String), Parts(Vec<serde_json::Value>) }
impl MessageContent { pub fn as_str(&self) -> &str; }
```

From src/router/selector.rs:
```rust
pub struct SelectedProvider {
    pub name: String, pub url: String, pub api_key: Option<ApiKey>,
    pub input_rate: u64, pub output_rate: u64, pub base_fee: u64,
}
pub fn actual_cost_sats(input_tokens: u32, output_tokens: u32, input_rate: u64, output_rate: u64, base_fee: u64) -> f64;
// Router::select(&self, model: &str, policy_name: Option<&str>, prompt: Option<&str>) -> Result<SelectedProvider>
```

From src/proxy/server.rs:
```rust
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub config: Arc<Config>,
    // ...
}
// Route registration pattern:
//   .route("/v1/chat/completions", post(handlers::chat_completions))
// Auth-protected routes go in `proxy_routes` block.
```

From src/proxy/handlers.rs:
```rust
pub const ARBSTR_POLICY_HEADER: &str = "x-arbstr-policy";
// Handler signature pattern: pub async fn handler(State(state): State<AppState>, ...) -> impl IntoResponse
```

From tests/common/mod.rs:
```rust
pub fn setup_circuit_test_app(providers: Vec<ProviderConfig>) -> (axum::Router, Arc<CircuitBreakerRegistry>);
pub async fn parse_body(response: axum::response::Response) -> (http::StatusCode, serde_json::Value);
pub fn test_provider(name: &str) -> ProviderConfig;
```
</interfaces>
</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Add /v1/cost handler and route</name>
  <files>src/proxy/handlers.rs, src/proxy/server.rs</files>
  <behavior>
    - Test: POST /v1/cost with valid body returns 200 with JSON containing `provider`, `model`, `estimated_input_tokens`, `estimated_output_tokens`, `estimated_cost_sats`, and rate breakdown
    - Test: POST /v1/cost with unknown model returns 400 error
    - Test: POST /v1/cost with X-Arbstr-Policy header selects provider per policy constraints
    - Test: POST /v1/cost picks cheapest provider (same as chat_completions routing)
    - Test: POST /v1/cost uses max_tokens from request body for output estimate (or default 256 if absent)
    - Test: POST /v1/cost with auth_token configured but missing bearer returns 401
  </behavior>
  <action>
1. In `src/proxy/handlers.rs`, add a `cost_estimate` handler:

```rust
/// Handle POST /v1/cost - estimate request cost without proxying.
///
/// Accepts the same ChatCompletionRequest body as /v1/chat/completions.
/// Returns the selected provider, estimated token counts, and cost in sats.
/// Does NOT call any upstream provider.
pub async fn cost_estimate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<impl IntoResponse, Error> {
```

Token estimation strategy:
- **Input tokens**: Estimate from message content using a ~4 chars/token heuristic. Sum all message content lengths, divide by 4, round up. This is a rough estimate since we have no tokenizer, but it is useful for cost previews.
- **Output tokens**: Use `request.max_tokens` if present, otherwise default to 256 as a reasonable preview estimate.

The handler should:
a. Extract policy name from `X-Arbstr-Policy` header (same as chat_completions).
b. Extract user prompt for heuristic policy matching via `request.user_prompt()`.
c. Call `state.router.select(&request.model, policy_name, prompt)` to get the cheapest provider.
d. Estimate input tokens from messages (sum all `message.content.as_str().len()`, divide by 4, minimum 1).
e. Use `max_tokens.unwrap_or(256)` for estimated output tokens.
f. Calculate cost via `crate::router::actual_cost_sats(input, output, provider.input_rate, provider.output_rate, provider.base_fee)`.
g. Return JSON response:
```json
{
  "model": "gpt-4o",
  "provider": "provider-alpha",
  "estimated_input_tokens": 42,
  "estimated_output_tokens": 256,
  "estimated_cost_sats": 8.12,
  "rates": {
    "input_rate_sats_per_1k": 10,
    "output_rate_sats_per_1k": 30,
    "base_fee_sats": 1
  }
}
```

2. In `src/proxy/server.rs`, register the route inside the `proxy_routes` block (so it gets auth protection when configured):
```rust
let proxy_routes = Router::new()
    .route("/v1/chat/completions", post(handlers::chat_completions))
    .route("/v1/models", get(handlers::list_models))
    .route("/v1/cost", post(handlers::cost_estimate));
```

3. In `src/proxy/handlers.rs`, add `pub use` for the handler if needed (check if list_models uses a direct function reference or goes through `handlers::` -- it uses `handlers::` directly, so no re-export needed; just make the function `pub`).
  </action>
  <verify>
    <automated>cd /home/john/vault/projects/github.com/arbstr && cargo build 2>&1 | tail -5</automated>
  </verify>
  <done>POST /v1/cost endpoint compiles and is registered in the router. Handler performs provider selection and cost estimation without calling upstream. Route is auth-protected when auth_token is configured.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Integration tests for /v1/cost</name>
  <files>tests/cost.rs</files>
  <behavior>
    - test_cost_basic: POST /v1/cost with model "gpt-4o" returns 200, response has all expected fields (provider, model, estimated_input_tokens, estimated_output_tokens, estimated_cost_sats, rates)
    - test_cost_unknown_model: POST /v1/cost with model "nonexistent" returns 400
    - test_cost_picks_cheapest: With two providers at different rates, /v1/cost returns the cheaper one
    - test_cost_max_tokens_used: When max_tokens=100 is set, estimated_output_tokens=100
    - test_cost_default_output_tokens: When max_tokens is absent, estimated_output_tokens=256
    - test_cost_with_policy_header: X-Arbstr-Policy header constrains provider selection
    - test_cost_input_estimation: Message content length affects estimated_input_tokens
    - test_cost_rates_in_response: rates object contains correct input_rate, output_rate, base_fee from selected provider
  </behavior>
  <action>
Create `tests/cost.rs` following the same pattern as `tests/health.rs`:

```rust
mod common;

use axum::body::Body;
use http::Request;
use tower::ServiceExt;
use arbstr::config::ProviderConfig;
```

Use `common::setup_circuit_test_app` to build test apps with custom provider configs. Use `common::parse_body` to extract JSON responses.

For each test:
- Build a minimal `ChatCompletionRequest`-shaped JSON body with the relevant fields.
- Send `POST /v1/cost` with `Content-Type: application/json`.
- Assert on status code and response JSON fields.

For `test_cost_picks_cheapest`: Configure two providers ("cheap" with output_rate=10, "expensive" with output_rate=30) both supporting "gpt-4o". Assert response.provider == "cheap".

For `test_cost_with_policy_header`: Add a policy rule with `max_sats_per_1k_output = 20` and two providers (one above, one below). Set `X-Arbstr-Policy` header. Assert correct provider selected.

For `test_cost_input_estimation`: Send a message with known content length (e.g., 400 chars = ~100 estimated tokens). Assert `estimated_input_tokens` is in a reasonable range (90-110).

Build the test app with custom providers inline (not using `db_test_config`) so rates are controlled and assertions are deterministic.
  </action>
  <verify>
    <automated>cd /home/john/vault/projects/github.com/arbstr && cargo test --test cost -- --nocapture 2>&1 | tail -20</automated>
  </verify>
  <done>All integration tests pass. Tests cover: basic response shape, unknown model error, cheapest provider selection, max_tokens handling, default output tokens, policy header, input token estimation, and rate breakdown accuracy.</done>
</task>

</tasks>

<verification>
```bash
# Full test suite passes (existing + new)
cd /home/john/vault/projects/github.com/arbstr && cargo test 2>&1 | tail -10

# Clippy clean
cargo clippy -- -D warnings 2>&1 | tail -5

# Format check
cargo fmt --check 2>&1 | tail -5
```
</verification>

<success_criteria>
- POST /v1/cost returns 200 with provider name, model, estimated token counts, estimated cost in sats, and rate breakdown
- POST /v1/cost with unknown model returns 400
- POST /v1/cost is auth-protected when auth_token is configured (sits behind same auth middleware as /v1/chat/completions)
- All existing tests continue to pass
- 8+ integration tests for the new endpoint pass
- cargo clippy clean, cargo fmt clean
</success_criteria>

<output>
After completion, create `.planning/quick/1-add-v1-cost-endpoint-for-request-cost-es/1-SUMMARY.md`
</output>
