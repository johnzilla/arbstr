//! Integration tests for circuit breaker routing behavior.
//!
//! Verifies that:
//! - Requests skip providers with open circuits
//! - 503 is returned when all provider circuits are open
//! - Circuit failures are recorded on 5xx responses
//! - Circuit state is not affected by 4xx responses
//! - Circuit success is recorded on 2xx responses
//!
//! Uses lightweight mock HTTP servers (axum on random ports) as fake
//! providers, and `tower::ServiceExt::oneshot` for the arbstr router.

use std::sync::Arc;

use axum::body::Body;
use http::Request;
use tower::ServiceExt;

use arbstr::config::{Config, PoliciesConfig, ProviderConfig, ServerConfig};
use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry, CircuitState};
use arbstr::router::Router as ProviderRouter;

/// Number of failures needed to trip a circuit (matches FAILURE_THRESHOLD in circuit_breaker.rs).
const FAILURE_THRESHOLD: u32 = 3;

/// Start a mock provider HTTP server that returns a valid chat completion response.
/// Returns the base URL (e.g., "http://127.0.0.1:12345/v1").
async fn start_mock_provider_ok() -> String {
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

/// Start a mock provider HTTP server that always returns 500.
async fn start_mock_provider_500() -> String {
    use axum::{http::StatusCode, routing::post, Router};

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider 500");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    format!("http://127.0.0.1:{}/v1", addr.port())
}

/// Start a mock provider HTTP server that always returns 400.
async fn start_mock_provider_400() -> String {
    use axum::{http::StatusCode, routing::post, Json, Router};

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {"message": "bad request", "type": "invalid_request_error"}
                })),
            )
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock provider 400");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    format!("http://127.0.0.1:{}/v1", addr.port())
}

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

/// Build a non-streaming chat completion request body.
fn chat_request_body(stream: bool) -> String {
    serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hello"}],
        "stream": stream
    })
    .to_string()
}

/// Parse the response body as JSON.
async fn parse_body(response: axum::response::Response) -> (http::StatusCode, serde_json::Value) {
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    (status, json)
}

// ============================================================================
// Test 1: Non-streaming 503 when all circuits are open
// ============================================================================

#[tokio::test]
async fn test_non_streaming_503_all_circuits_open() {
    let providers = vec![
        ProviderConfig {
            name: "provider-a".to_string(),
            url: "https://fake-a.test/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 5,
            output_rate: 15,
            base_fee: 0,
        },
        ProviderConfig {
            name: "provider-b".to_string(),
            url: "https://fake-b.test/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
        },
    ];

    let (app, registry) = setup_circuit_test_app(providers);

    // Trip both circuits
    trip_circuit(&registry, "provider-a");
    trip_circuit(&registry, "provider-b");

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(false)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, body) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("open circuits"),
        "Body should mention open circuits: {:?}",
        body
    );
}

// ============================================================================
// Test 2: Non-streaming skips open circuit, routes to next provider
// ============================================================================

#[tokio::test]
async fn test_non_streaming_skips_open_circuit() {
    let mock_url = start_mock_provider_ok().await;

    let providers = vec![
        ProviderConfig {
            name: "provider-a".to_string(),
            url: "https://fake-a.test/v1".to_string(), // unreachable, but circuit is open
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 3, // cheaper, would be selected first
            output_rate: 10,
            base_fee: 0,
        },
        ProviderConfig {
            name: "provider-b".to_string(),
            url: mock_url,
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
        },
    ];

    let (app, registry) = setup_circuit_test_app(providers);

    // Trip only provider-a's circuit
    trip_circuit(&registry, "provider-a");

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(false)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-arbstr-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "provider-b"
    );
}

// ============================================================================
// Test 3: Streaming 503 when all circuits are open
// ============================================================================

#[tokio::test]
async fn test_streaming_503_all_circuits_open() {
    let providers = vec![
        ProviderConfig {
            name: "provider-a".to_string(),
            url: "https://fake-a.test/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 5,
            output_rate: 15,
            base_fee: 0,
        },
        ProviderConfig {
            name: "provider-b".to_string(),
            url: "https://fake-b.test/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
        },
    ];

    let (app, registry) = setup_circuit_test_app(providers);

    // Trip both circuits
    trip_circuit(&registry, "provider-a");
    trip_circuit(&registry, "provider-b");

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(true)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, body) = parse_body(response).await;

    assert_eq!(status, http::StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("open circuits"),
        "Body should mention open circuits: {:?}",
        body
    );
}

// ============================================================================
// Test 4: Streaming skips open circuit, routes to next provider
// ============================================================================

#[tokio::test]
async fn test_streaming_skips_open_circuit() {
    let mock_url = start_mock_provider_ok().await;

    let providers = vec![
        ProviderConfig {
            name: "provider-a".to_string(),
            url: "https://fake-a.test/v1".to_string(), // unreachable, circuit open
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 3, // cheaper
            output_rate: 10,
            base_fee: 0,
        },
        ProviderConfig {
            name: "provider-b".to_string(),
            url: mock_url,
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
        },
    ];

    let (app, registry) = setup_circuit_test_app(providers);

    // Trip only provider-a's circuit
    trip_circuit(&registry, "provider-a");

    // Streaming request -- send_to_provider will get a non-SSE JSON response from mock,
    // but since the mock returns 200, the streaming path treats it as a success.
    // The provider header is set before the body streams.
    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(true)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should succeed via provider-b (provider-a's circuit is open)
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-arbstr-provider")
            .unwrap()
            .to_str()
            .unwrap(),
        "provider-b"
    );
}

// ============================================================================
// Test 5: Circuit records failure on 5xx response
// ============================================================================

#[tokio::test]
async fn test_circuit_records_failure_on_5xx() {
    let mock_url = start_mock_provider_500().await;

    let providers = vec![ProviderConfig {
        name: "provider-a".to_string(),
        url: mock_url,
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
    }];

    let (app, registry) = setup_circuit_test_app(providers);

    // Verify initial state
    assert_eq!(registry.failure_count("provider-a"), Some(0));

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(false)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should fail (provider returned 500)
    assert_ne!(response.status(), http::StatusCode::OK);

    // Circuit should have recorded the failure
    // Non-streaming path records per-attempt failures in the retry loop
    let failure_count = registry.failure_count("provider-a").unwrap();
    assert!(
        failure_count >= 1,
        "Expected at least 1 failure recorded, got {}",
        failure_count
    );
}

// ============================================================================
// Test 6: Circuit stays closed on 4xx response
// ============================================================================

#[tokio::test]
async fn test_circuit_stays_closed_on_4xx() {
    let mock_url = start_mock_provider_400().await;

    let providers = vec![ProviderConfig {
        name: "provider-a".to_string(),
        url: mock_url,
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
    }];

    let (app, registry) = setup_circuit_test_app(providers);

    assert_eq!(registry.failure_count("provider-a"), Some(0));

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(false)))
        .unwrap();

    let _response = app.oneshot(request).await.unwrap();

    // 4xx should NOT increment circuit failure count
    assert_eq!(
        registry.failure_count("provider-a"),
        Some(0),
        "4xx responses should not increment circuit failure count"
    );
    assert_eq!(
        registry.state("provider-a"),
        Some(CircuitState::Closed),
        "Circuit should remain Closed after 4xx"
    );
}

// ============================================================================
// Test 7: Non-streaming records circuit success on 2xx
// ============================================================================

#[tokio::test]
async fn test_non_streaming_records_success() {
    let mock_url = start_mock_provider_ok().await;

    let providers = vec![ProviderConfig {
        name: "provider-a".to_string(),
        url: mock_url,
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
    }];

    let (app, registry) = setup_circuit_test_app(providers);

    // Add 2 failures (below threshold) to verify success resets them
    registry.record_failure("provider-a", "5xx", "Error 1");
    registry.record_failure("provider-a", "5xx", "Error 2");
    assert_eq!(registry.failure_count("provider-a"), Some(2));

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(false)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);

    // record_success should have reset failure count to 0
    assert_eq!(
        registry.failure_count("provider-a"),
        Some(0),
        "Success should reset failure count"
    );
    assert_eq!(
        registry.state("provider-a"),
        Some(CircuitState::Closed),
        "Circuit should still be Closed"
    );
}

// ============================================================================
// Test 8: Streaming records circuit failure on 5xx
// ============================================================================

#[tokio::test]
async fn test_streaming_records_failure_on_5xx() {
    let mock_url = start_mock_provider_500().await;

    let providers = vec![ProviderConfig {
        name: "provider-a".to_string(),
        url: mock_url,
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
    }];

    let (app, registry) = setup_circuit_test_app(providers);

    assert_eq!(registry.failure_count("provider-a"), Some(0));

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(true)))
        .unwrap();

    let _response = app.oneshot(request).await.unwrap();

    // Streaming path should record 5xx as circuit failure
    let failure_count = registry.failure_count("provider-a").unwrap();
    assert!(
        failure_count >= 1,
        "Expected at least 1 failure recorded for streaming 5xx, got {}",
        failure_count
    );
}

// ============================================================================
// Test 9: Request ID header present on circuit-open 503
// ============================================================================

#[tokio::test]
async fn test_503_has_request_id_header() {
    let providers = vec![ProviderConfig {
        name: "provider-a".to_string(),
        url: "https://fake.test/v1".to_string(),
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
    }];

    let (app, registry) = setup_circuit_test_app(providers);
    trip_circuit(&registry, "provider-a");

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(chat_request_body(false)))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        response.headers().contains_key("x-arbstr-request-id"),
        "503 response should include x-arbstr-request-id header"
    );
}
