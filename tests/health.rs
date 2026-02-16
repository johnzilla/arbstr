//! Integration tests for the enhanced /health endpoint.
//!
//! Verifies that:
//! - GET /health returns per-provider circuit breaker state
//! - Top-level status is "ok" when all circuits are closed
//! - Top-level status is "degraded" when some circuits are open or half-open
//! - Top-level status is "unhealthy" (HTTP 503) when ALL circuits are open
//! - Zero configured providers returns "ok" with empty providers object
//! - Half-open providers count as degraded, not unhealthy
//! - Failure count is accurately reported

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use http::Request;
use tower::ServiceExt;

use arbstr::config::{Config, PoliciesConfig, ProviderConfig, ServerConfig};
use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry, CircuitState};
use arbstr::router::Router as ProviderRouter;

/// Number of failures needed to trip a circuit (matches FAILURE_THRESHOLD in circuit_breaker.rs).
const FAILURE_THRESHOLD: u32 = 3;

/// Build an arbstr test app with custom providers and return the router + registry.
fn setup_circuit_test_app(
    providers: Vec<ProviderConfig>,
) -> (axum::Router, Arc<CircuitBreakerRegistry>) {
    let provider_names: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let registry = Arc::new(CircuitBreakerRegistry::new(&provider_names));

    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
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
        circuit_breakers: registry.clone(),
    };

    let app = create_router(state);
    (app, registry)
}

/// Trip a provider's circuit by recording FAILURE_THRESHOLD consecutive failures.
fn trip_circuit(registry: &CircuitBreakerRegistry, provider: &str) {
    for _ in 0..FAILURE_THRESHOLD {
        registry.record_failure(provider, "5xx", "Internal Server Error");
    }
}

/// Parse the response body as JSON and return (status_code, json_value).
async fn parse_body(response: axum::response::Response) -> (http::StatusCode, serde_json::Value) {
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    (status, json)
}

/// Standard provider config for tests.
fn test_provider(name: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        url: "https://fake.test/v1".to_string(),
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
    }
}

// ============================================================================
// Test 1: All circuits closed -> "ok" (HTTP 200)
// ============================================================================

#[tokio::test]
async fn test_health_ok_all_closed() {
    let providers = vec![test_provider("provider-a"), test_provider("provider-b")];
    let (app, _registry) = setup_circuit_test_app(providers);

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["status"], "ok");

    // Both providers present with closed state
    let pa = &json["providers"]["provider-a"];
    assert_eq!(pa["state"], "closed");
    assert_eq!(pa["failure_count"], 0);

    let pb = &json["providers"]["provider-b"];
    assert_eq!(pb["state"], "closed");
    assert_eq!(pb["failure_count"], 0);
}

// ============================================================================
// Test 2: Zero providers -> "ok" (HTTP 200) with empty providers
// ============================================================================

#[tokio::test]
async fn test_health_ok_zero_providers() {
    let (app, _registry) = setup_circuit_test_app(vec![]);

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(
        json["providers"].as_object().unwrap().len(),
        0,
        "providers should be empty object"
    );
}

// ============================================================================
// Test 3: One circuit open, one closed -> "degraded" (HTTP 200)
// ============================================================================

#[tokio::test]
async fn test_health_degraded_one_open() {
    let providers = vec![test_provider("provider-a"), test_provider("provider-b")];
    let (app, registry) = setup_circuit_test_app(providers);

    // Trip only provider-a
    trip_circuit(&registry, "provider-a");

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["status"], "degraded");

    let pa = &json["providers"]["provider-a"];
    assert_eq!(pa["state"], "open");
    assert_eq!(pa["failure_count"], 3);

    let pb = &json["providers"]["provider-b"];
    assert_eq!(pb["state"], "closed");
    assert_eq!(pb["failure_count"], 0);
}

// ============================================================================
// Test 4: All circuits open -> "unhealthy" (HTTP 503)
// ============================================================================

#[tokio::test]
async fn test_health_unhealthy_all_open() {
    let providers = vec![test_provider("provider-a"), test_provider("provider-b")];
    let (app, registry) = setup_circuit_test_app(providers);

    // Trip both circuits
    trip_circuit(&registry, "provider-a");
    trip_circuit(&registry, "provider-b");

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["status"], "unhealthy");

    assert_eq!(json["providers"]["provider-a"]["state"], "open");
    assert_eq!(json["providers"]["provider-b"]["state"], "open");
}

// ============================================================================
// Test 5: Half-open provider -> "degraded" (HTTP 200)
// ============================================================================

#[tokio::test(start_paused = true)]
async fn test_health_degraded_half_open() {
    let providers = vec![test_provider("provider-a")];
    let (app, registry) = setup_circuit_test_app(providers);

    // Trip the circuit
    trip_circuit(&registry, "provider-a");
    assert_eq!(registry.state("provider-a"), Some(CircuitState::Open));

    // Advance time past the 30s timeout
    tokio::time::advance(Duration::from_secs(31)).await;

    // Trigger lazy Open -> HalfOpen transition via acquire_permit
    let permit = registry.acquire_permit("provider-a").await;
    assert!(permit.is_ok(), "Should get probe permit after timeout");
    assert_eq!(registry.state("provider-a"), Some(CircuitState::HalfOpen));

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["status"], "degraded");
    assert_eq!(json["providers"]["provider-a"]["state"], "half_open");
}

// ============================================================================
// Test 6: Mix of open and half-open -> "degraded" (not unhealthy)
// ============================================================================

#[tokio::test(start_paused = true)]
async fn test_health_degraded_mix_open_half_open() {
    let providers = vec![test_provider("provider-a"), test_provider("provider-b")];
    let (app, registry) = setup_circuit_test_app(providers);

    // Trip both circuits
    trip_circuit(&registry, "provider-a");
    trip_circuit(&registry, "provider-b");

    // Advance time past timeout
    tokio::time::advance(Duration::from_secs(31)).await;

    // Transition provider-a to HalfOpen via acquire_permit
    let permit = registry.acquire_permit("provider-a").await;
    assert!(permit.is_ok());
    assert_eq!(registry.state("provider-a"), Some(CircuitState::HalfOpen));
    // provider-b also transitions to HalfOpen on acquire_permit (timeout expired for both)
    // We need to check that provider-b is also half-open or still open
    // Since we called acquire_permit only on provider-a, provider-b stays Open
    // until someone calls acquire_permit on it (lazy transition).
    assert_eq!(registry.state("provider-b"), Some(CircuitState::Open));

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(
        json["status"], "degraded",
        "Mix of open and half-open should be degraded, not unhealthy"
    );

    assert_eq!(json["providers"]["provider-a"]["state"], "half_open");
    assert_eq!(json["providers"]["provider-b"]["state"], "open");
}

// ============================================================================
// Test 7: Single provider open -> "unhealthy" (HTTP 503)
// ============================================================================

#[tokio::test]
async fn test_health_single_provider_open() {
    let providers = vec![test_provider("provider-a")];
    let (app, registry) = setup_circuit_test_app(providers);

    trip_circuit(&registry, "provider-a");

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json["status"], "unhealthy");
    assert_eq!(json["providers"]["provider-a"]["state"], "open");
}

// ============================================================================
// Test 8: Failure count increments below threshold
// ============================================================================

#[tokio::test]
async fn test_health_failure_count_increments() {
    let providers = vec![test_provider("provider-a")];
    let (app, registry) = setup_circuit_test_app(providers);

    // Record 2 failures (below threshold of 3)
    registry.record_failure("provider-a", "5xx", "Error 1");
    registry.record_failure("provider-a", "5xx", "Error 2");

    let request = Request::get("/health").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let (status, json) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["providers"]["provider-a"]["state"], "closed");
    assert_eq!(json["providers"]["provider-a"]["failure_count"], 2);
}
