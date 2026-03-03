//! Integration tests for the POST /v1/cost endpoint.
//!
//! Verifies that:
//! - POST /v1/cost returns 200 with provider, model, token estimates, cost, and rates
//! - POST /v1/cost with unknown model returns 400
//! - POST /v1/cost picks the cheapest provider
//! - POST /v1/cost uses max_tokens from request body for output estimate
//! - POST /v1/cost defaults to 256 output tokens when max_tokens absent
//! - POST /v1/cost respects X-Arbstr-Policy header for provider selection
//! - POST /v1/cost estimates input tokens from message content length
//! - POST /v1/cost rates object matches selected provider's configured rates

mod common;

use std::sync::Arc;

use axum::body::Body;
use http::Request;
use tower::ServiceExt;

use arbstr::config::{Config, PoliciesConfig, PolicyRule, ProviderConfig, ServerConfig};
use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry};
use arbstr::router::Router as ProviderRouter;

/// Build a test app with custom providers and optional policy rules.
fn setup_cost_test_app(
    providers: Vec<ProviderConfig>,
    policy_rules: Vec<PolicyRule>,
) -> axum::Router {
    let provider_names: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let registry = Arc::new(CircuitBreakerRegistry::new(&provider_names));

    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: None,
        },
        database: None,
        providers: providers.clone(),
        policies: PoliciesConfig {
            default_strategy: "cheapest".to_string(),
            rules: policy_rules.clone(),
        },
        logging: Default::default(),
    };

    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );

    let state = AppState {
        router: Arc::new(provider_router),
        http_client: reqwest::Client::new(),
        config: Arc::new(config),
        db: None,
        read_db: None,
        db_writer: None,
        circuit_breakers: registry,
    };

    create_router(state)
}

/// Build a test app with auth token configured.
fn setup_cost_test_app_with_auth(providers: Vec<ProviderConfig>, auth_token: &str) -> axum::Router {
    let provider_names: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let registry = Arc::new(CircuitBreakerRegistry::new(&provider_names));

    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: Some(auth_token.to_string()),
        },
        database: None,
        providers: providers.clone(),
        policies: PoliciesConfig::default(),
        logging: Default::default(),
    };

    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );

    let state = AppState {
        router: Arc::new(provider_router),
        http_client: reqwest::Client::new(),
        config: Arc::new(config),
        db: None,
        read_db: None,
        db_writer: None,
        circuit_breakers: registry,
    };

    create_router(state)
}

/// Standard provider config with controllable rates.
fn provider_with_rates(
    name: &str,
    input_rate: u64,
    output_rate: u64,
    base_fee: u64,
) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        url: "https://fake.test/v1".to_string(),
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate,
        output_rate,
        base_fee,
    }
}

/// Build a minimal JSON body for POST /v1/cost.
fn cost_request_body(model: &str, message: &str, max_tokens: Option<u32>) -> String {
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": message
            }
        ]
    });

    if let Some(mt) = max_tokens {
        body["max_tokens"] = serde_json::json!(mt);
    }

    serde_json::to_string(&body).unwrap()
}

// ============================================================================
// Test 1: Basic response shape (200 with all expected fields)
// ============================================================================

#[tokio::test]
async fn test_cost_basic() {
    let providers = vec![provider_with_rates("alpha", 10, 30, 1)];
    let app = setup_cost_test_app(providers, vec![]);

    let body = cost_request_body("gpt-4o", "Hello, world!", None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["model"], "gpt-4o");
    assert_eq!(json["provider"], "alpha");
    assert!(
        json["estimated_input_tokens"].is_number(),
        "expected estimated_input_tokens to be a number"
    );
    assert!(
        json["estimated_output_tokens"].is_number(),
        "expected estimated_output_tokens to be a number"
    );
    assert!(
        json["estimated_cost_sats"].is_number(),
        "expected estimated_cost_sats to be a number"
    );
    assert!(json["rates"].is_object(), "expected rates to be an object");
    assert_eq!(json["rates"]["input_rate_sats_per_1k"], 10);
    assert_eq!(json["rates"]["output_rate_sats_per_1k"], 30);
    assert_eq!(json["rates"]["base_fee_sats"], 1);
}

// ============================================================================
// Test 2: Unknown model returns 400
// ============================================================================

#[tokio::test]
async fn test_cost_unknown_model() {
    let providers = vec![provider_with_rates("alpha", 10, 30, 1)];
    let app = setup_cost_test_app(providers, vec![]);

    let body = cost_request_body("nonexistent-model", "hello", None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, _json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::BAD_REQUEST);
}

// ============================================================================
// Test 3: Picks cheapest provider
// ============================================================================

#[tokio::test]
async fn test_cost_picks_cheapest() {
    let providers = vec![
        provider_with_rates("expensive", 15, 40, 5),
        provider_with_rates("cheap", 3, 10, 0),
    ];
    let app = setup_cost_test_app(providers, vec![]);

    let body = cost_request_body("gpt-4o", "hello", None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        json["provider"], "cheap",
        "should select the cheapest provider"
    );
}

// ============================================================================
// Test 4: max_tokens used for output estimate
// ============================================================================

#[tokio::test]
async fn test_cost_max_tokens_used() {
    let providers = vec![provider_with_rates("alpha", 10, 30, 0)];
    let app = setup_cost_test_app(providers, vec![]);

    let body = cost_request_body("gpt-4o", "hello", Some(100));
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["estimated_output_tokens"], 100);
}

// ============================================================================
// Test 5: Default output tokens (256) when max_tokens absent
// ============================================================================

#[tokio::test]
async fn test_cost_default_output_tokens() {
    let providers = vec![provider_with_rates("alpha", 10, 30, 0)];
    let app = setup_cost_test_app(providers, vec![]);

    let body = cost_request_body("gpt-4o", "hello", None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["estimated_output_tokens"], 256);
}

// ============================================================================
// Test 6: X-Arbstr-Policy header constrains provider selection
// ============================================================================

#[tokio::test]
async fn test_cost_with_policy_header() {
    // "expensive" has output_rate=40 (above max_sats_per_1k_output=20)
    // "budget" has output_rate=10 (below max_sats_per_1k_output=20)
    let providers = vec![
        provider_with_rates("expensive", 15, 40, 0),
        provider_with_rates("budget", 3, 10, 0),
    ];

    let policy = PolicyRule {
        name: "strict_budget".to_string(),
        allowed_models: vec!["gpt-4o".to_string()],
        strategy: "lowest_cost".to_string(),
        max_sats_per_1k_output: Some(20),
        keywords: vec![],
    };

    let app = setup_cost_test_app(providers, vec![policy]);

    let body = cost_request_body("gpt-4o", "hello", None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .header("x-arbstr-policy", "strict_budget")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        json["provider"], "budget",
        "policy should filter out expensive provider (output_rate 40 > max 20)"
    );
}

// ============================================================================
// Test 7: Input token estimation from message content length
// ============================================================================

#[tokio::test]
async fn test_cost_input_estimation() {
    let providers = vec![provider_with_rates("alpha", 10, 30, 0)];
    let app = setup_cost_test_app(providers, vec![]);

    // 400 characters of content -> ~100 estimated tokens (400/4)
    let long_message = "a".repeat(400);
    let body = cost_request_body("gpt-4o", &long_message, None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    let input_tokens = json["estimated_input_tokens"].as_u64().unwrap();
    assert_eq!(input_tokens, 100, "400 chars / 4 = 100 estimated tokens");
}

// ============================================================================
// Test 8: Rates object matches selected provider
// ============================================================================

#[tokio::test]
async fn test_cost_rates_in_response() {
    let providers = vec![provider_with_rates("alpha", 7, 22, 3)];
    let app = setup_cost_test_app(providers, vec![]);

    let body = cost_request_body("gpt-4o", "hello", None);
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["rates"]["input_rate_sats_per_1k"], 7);
    assert_eq!(json["rates"]["output_rate_sats_per_1k"], 22);
    assert_eq!(json["rates"]["base_fee_sats"], 3);
}

// ============================================================================
// Test 9: Auth required when auth_token is configured
// ============================================================================

#[tokio::test]
async fn test_cost_requires_auth() {
    let providers = vec![provider_with_rates("alpha", 10, 30, 0)];
    let app = setup_cost_test_app_with_auth(providers, "secret-token-123");

    let body = cost_request_body("gpt-4o", "hello", None);

    // Request without bearer token should get 401
    let request = Request::post("/v1/cost")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, _json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
}
