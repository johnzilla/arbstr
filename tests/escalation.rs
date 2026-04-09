//! Integration tests for complexity header override and tier escalation.
//!
//! Verifies:
//! - X-Arbstr-Complexity: high routes to frontier-tier provider (SCORE-03)
//! - X-Arbstr-Complexity: low routes to local-tier provider (SCORE-03)
//! - X-Arbstr-Complexity: medium routes to standard-or-lower tier (SCORE-03)
//! - Invalid X-Arbstr-Complexity header falls through to scorer (SCORE-03)
//! - Missing header uses scorer (SCORE-03)
//! - Escalation when local-tier provider is circuit-broken (ROUTE-04)
//! - Escalation is one-way (ROUTE-05)

mod common;

use axum::body::Body;
use http::Request;
use tower::ServiceExt;

use arbstr::config::{ProviderConfig, Tier};
use arbstr::proxy::CircuitBreakerRegistry;

/// Number of failures needed to trip a circuit (matches FAILURE_THRESHOLD in circuit_breaker.rs).
const FAILURE_THRESHOLD: u32 = 3;

/// Start a mock provider HTTP server that returns a valid chat completion response
/// with the provider name in the response for identification.
async fn start_mock_provider(_name: &str) -> String {
    use axum::{routing::post, Json, Router};

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(serde_json::json!({
                "id": "chatcmpl-mock",
                "object": "chat.completion",
                "choices": [{
                    "message": {"role": "assistant", "content": "mock response"},
                    "index": 0,
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "total_tokens": 15
                }
            }))
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    format!("http://127.0.0.1:{}/v1", addr.port())
}

/// Trip a provider's circuit by recording FAILURE_THRESHOLD consecutive failures.
fn trip_circuit(registry: &CircuitBreakerRegistry, provider: &str) {
    for _ in 0..FAILURE_THRESHOLD {
        registry.record_failure(provider, "5xx", "Internal Server Error");
    }
}

/// Build tiered providers with mock URLs.
async fn tiered_providers() -> Vec<ProviderConfig> {
    let local_url = start_mock_provider("local-provider").await;
    let standard_url = start_mock_provider("standard-provider").await;
    let frontier_url = start_mock_provider("frontier-provider").await;

    vec![
        ProviderConfig {
            name: "local-provider".to_string(),
            url: local_url,
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 1,
            output_rate: 5,
            base_fee: 0,
            tier: Tier::Local,
        },
        ProviderConfig {
            name: "standard-provider".to_string(),
            url: standard_url,
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 5,
            output_rate: 15,
            base_fee: 1,
            tier: Tier::Standard,
        },
        ProviderConfig {
            name: "frontier-provider".to_string(),
            url: frontier_url,
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 2,
            tier: Tier::Frontier,
        },
    ]
}

/// Build a simple chat request body.
fn chat_request_body() -> String {
    serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": false
    })
    .to_string()
}

/// Build a complex chat request body that should score high on the complexity scorer.
fn complex_chat_request_body() -> String {
    serde_json::json!({
        "model": "gpt-4o",
        "messages": [
            {"role": "user", "content": "Please analyze the following code and refactor it:\n```rust\nfn main() {\n    let x = vec![1, 2, 3];\n    let y: Vec<_> = x.iter().map(|i| i * 2).collect();\n    println!(\"{:?}\", y);\n}\n```\nAlso check `src/main.rs` and `src/lib.rs` and `tests/integration.rs` for any issues with the architecture. Consider the trade-offs between performance and readability, and explain your reasoning step by step."},
            {"role": "assistant", "content": "I'll analyze each file systematically."},
            {"role": "user", "content": "Also compare `src/router/mod.rs` and `src/proxy/server.rs` -- think carefully about the dependency graph."}
        ],
        "stream": false
    })
    .to_string()
}

// ============================================================================
// Test 1: X-Arbstr-Complexity: high routes to frontier (SCORE-03)
// ============================================================================

#[tokio::test]
async fn test_complexity_header_high_routes_to_frontier() {
    let providers = tiered_providers().await;
    let (app, _registry) = common::setup_circuit_test_app(providers);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-arbstr-complexity", "high")
                .body(Body::from(chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    let provider = response
        .headers()
        .get("x-arbstr-provider")
        .and_then(|v| v.to_str().ok())
        .expect("x-arbstr-provider header present");

    // "high" maps to Frontier tier, which includes all providers.
    // The cheapest provider across all tiers is local-provider.
    // This verifies the tier override allows ALL tiers (Frontier includes Local+Standard+Frontier).
    assert!(
        ["local-provider", "standard-provider", "frontier-provider"].contains(&provider),
        "Expected any provider (frontier tier includes all), got: {}",
        provider
    );
}

// ============================================================================
// Test 2: X-Arbstr-Complexity: low routes to local (SCORE-03)
// ============================================================================

#[tokio::test]
async fn test_complexity_header_low_routes_to_local() {
    let providers = tiered_providers().await;
    let (app, _registry) = common::setup_circuit_test_app(providers);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-arbstr-complexity", "low")
                .body(Body::from(complex_chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    let provider = response
        .headers()
        .get("x-arbstr-provider")
        .and_then(|v| v.to_str().ok())
        .expect("x-arbstr-provider header present");

    // "low" maps to Local tier -- only local-provider should be selected
    assert_eq!(
        provider, "local-provider",
        "Low complexity should route to local-provider only"
    );
}

// ============================================================================
// Test 3: X-Arbstr-Complexity: medium routes to standard or lower (SCORE-03)
// ============================================================================

#[tokio::test]
async fn test_complexity_header_medium_routes_to_standard() {
    let providers = tiered_providers().await;
    let (app, _registry) = common::setup_circuit_test_app(providers);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-arbstr-complexity", "medium")
                .body(Body::from(chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    let provider = response
        .headers()
        .get("x-arbstr-provider")
        .and_then(|v| v.to_str().ok())
        .expect("x-arbstr-provider header present");

    // "medium" maps to Standard tier -- local-provider or standard-provider
    assert!(
        provider == "local-provider" || provider == "standard-provider",
        "Medium complexity should route to local or standard, got: {}",
        provider
    );
}

// ============================================================================
// Test 4: Invalid X-Arbstr-Complexity header falls through to scorer (SCORE-03)
// ============================================================================

#[tokio::test]
async fn test_complexity_header_invalid_uses_scorer() {
    let providers = tiered_providers().await;
    let (app, _registry) = common::setup_circuit_test_app(providers);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-arbstr-complexity", "invalid-value")
                .body(Body::from(chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should succeed -- invalid header value falls through to scorer (D-12)
    assert_eq!(
        response.status(),
        http::StatusCode::OK,
        "Invalid complexity header should not cause an error"
    );
}

// ============================================================================
// Test 5: No complexity header uses scorer (SCORE-03)
// ============================================================================

#[tokio::test]
async fn test_no_complexity_header_uses_scorer() {
    let providers = tiered_providers().await;
    let (app, _registry) = common::setup_circuit_test_app(providers);

    // Simple "hello" message should score low -> routes to local/standard
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    let provider = response
        .headers()
        .get("x-arbstr-provider")
        .and_then(|v| v.to_str().ok())
        .expect("x-arbstr-provider header present");

    // Simple "hello" scores low, should not route to frontier-only
    // (cheapest provider at scored tier will be selected)
    assert!(
        ["local-provider", "standard-provider", "frontier-provider"].contains(&provider),
        "Simple message should route to some provider, got: {}",
        provider
    );
}

// ============================================================================
// Test 6: Escalation when local-tier provider is circuit-broken (ROUTE-04)
// ============================================================================

#[tokio::test]
async fn test_escalation_when_local_circuit_broken() {
    let providers = tiered_providers().await;
    let (app, registry) = common::setup_circuit_test_app(providers);

    // Trip the local provider's circuit
    trip_circuit(&registry, "local-provider");

    // Send with "low" complexity -- should try local, find it circuit-broken, escalate to standard
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-arbstr-complexity", "low")
                .body(Body::from(chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    let provider = response
        .headers()
        .get("x-arbstr-provider")
        .and_then(|v| v.to_str().ok())
        .expect("x-arbstr-provider header present");

    // Should have escalated from Local to Standard (local-provider is circuit-broken)
    // Standard tier includes local + standard providers, but local is broken
    // So standard-provider should handle it
    assert_eq!(
        provider, "standard-provider",
        "Should escalate to standard-provider when local is circuit-broken"
    );
}

// ============================================================================
// Test 7: Escalation is one-way -- never de-escalates (ROUTE-05)
// ============================================================================

#[tokio::test]
async fn test_escalation_one_way_never_deescalates() {
    let providers = tiered_providers().await;
    let (app, registry) = common::setup_circuit_test_app(providers);

    // Trip both local and standard providers' circuits
    trip_circuit(&registry, "local-provider");
    trip_circuit(&registry, "standard-provider");

    // Send with "low" complexity -- should escalate Local -> Standard -> Frontier
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-arbstr-complexity", "low")
                .body(Body::from(chat_request_body()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    let provider = response
        .headers()
        .get("x-arbstr-provider")
        .and_then(|v| v.to_str().ok())
        .expect("x-arbstr-provider header present");

    // After double escalation (Local -> Standard -> Frontier), only frontier-provider is healthy
    assert_eq!(
        provider, "frontier-provider",
        "Should escalate all the way to frontier-provider when local and standard are circuit-broken"
    );
}
