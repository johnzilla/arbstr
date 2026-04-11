//! Integration tests for vault billing flow.
//!
//! Uses a mock HTTP vault server and a mock provider server to verify:
//! - Bearer token required when vault is configured (401 without)
//! - Vault error codes (402, 403, 429) mapped to OpenAI-compatible format
//! - Frontier rates used for reserve amount estimation (BILL-05)
//! - Reserve happens BEFORE provider contact (BILL-02 ordering)
//! - Full reserve -> route -> settle path (BILL-03)
//! - Release on provider failure (BILL-04)
//! - Free proxy mode works without vault (BILL-08)
//! - Vault auth replaces server auth when vault configured (D-01)

mod common;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::routing::post;
use axum::Router;
use http::Request;
use tower::ServiceExt;

/// A recorded call to the mock vault or mock provider, with operation name,
/// request body, and timestamp for ordering assertions.
type CallLog = Arc<Mutex<Vec<(String, serde_json::Value, Instant)>>>;

/// Minimal chat completion request body.
fn chat_request_body() -> String {
    serde_json::to_string(&serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hello world test message"}]
    }))
    .unwrap()
}

/// Valid OpenAI-compatible chat completion response.
fn valid_completion_response() -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1234567890,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "test response"},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 20,
            "total_tokens": 30
        }
    })
}

/// Start a mock vault server on a random port. Returns (url, call_log).
///
/// The `error_override` parameter allows specific endpoints to return error codes
/// for testing vault error mapping. Format: Some((status_code,)) applies to /internal/reserve.
async fn start_mock_vault(error_override: Option<u16>) -> (String, CallLog) {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));

    let reserve_log = log.clone();
    let settle_log = log.clone();
    let release_log = log.clone();

    let app = Router::new()
        .route(
            "/internal/reserve",
            post({
                let log = reserve_log;
                let error_code = error_override;
                move |headers: axum::http::HeaderMap, body: axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        // Validate X-Internal-Token header
                        let token = headers
                            .get("X-Internal-Token")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("");
                        if token != "test-internal-token" {
                            return (
                                axum::http::StatusCode::FORBIDDEN,
                                axum::Json(serde_json::json!({"error": "invalid internal token"})),
                            );
                        }

                        log.lock().unwrap().push((
                            "reserve".to_string(),
                            body.0.clone(),
                            Instant::now(),
                        ));

                        if let Some(code) = error_code {
                            return (
                                axum::http::StatusCode::from_u16(code).unwrap(),
                                axum::Json(serde_json::json!({"error": "mock vault error"})),
                            );
                        }

                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({"reservation_id": "res-test-123"})),
                        )
                    }
                }
            }),
        )
        .route(
            "/internal/settle",
            post({
                let log = settle_log;
                move |body: axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push((
                            "settle".to_string(),
                            body.0.clone(),
                            Instant::now(),
                        ));
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({"settled": true, "refunded_msats": 0})),
                        )
                    }
                }
            }),
        )
        .route(
            "/internal/release",
            post({
                let log = release_log;
                move |body: axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push((
                            "release".to_string(),
                            body.0.clone(),
                            Instant::now(),
                        ));
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({"released": true})),
                        )
                    }
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (url, log)
}

/// Start a mock provider server on a random port. Returns (url, call_log).
///
/// If `fail` is true, the provider returns 500 for all requests.
async fn start_mock_provider(fail: bool) -> (String, CallLog) {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let provider_log = log.clone();

    let app = Router::new().route(
        "/v1/chat/completions",
        post({
            let log = provider_log;
            move |body: axum::Json<serde_json::Value>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push((
                        "chat_completion".to_string(),
                        body.0.clone(),
                        Instant::now(),
                    ));

                    if fail {
                        return (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            axum::Json(serde_json::json!({"error": {"message": "provider failure", "type": "server_error"}})),
                        );
                    }

                    (
                        axum::http::StatusCode::OK,
                        axum::Json(valid_completion_response()),
                    )
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (url, log)
}

// ============================================================================
// Test a: Bearer token required when vault is configured
// ============================================================================

#[tokio::test]
async fn test_vault_reserve_requires_bearer_token() {
    let (vault_url, _vault_log) = start_mock_vault(None).await;
    let (provider_url, _provider_log) = start_mock_provider(false).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    // Send request WITHOUT Authorization header
    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        json["error"]["message"], "Authorization: Bearer <token> required",
        "should return OpenAI-compatible error for missing bearer token"
    );
    assert_eq!(json["error"]["type"], "authentication_error");
}

// ============================================================================
// Test b: Vault 402 (insufficient balance)
// ============================================================================

#[tokio::test]
async fn test_vault_reserve_insufficient_balance() {
    let (vault_url, _vault_log) = start_mock_vault(Some(402)).await;
    let (provider_url, _provider_log) = start_mock_provider(false).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status.as_u16(), 402);
    assert_eq!(json["error"]["type"], "billing_error");
}

// ============================================================================
// Test c: Vault 403 (policy denied)
// ============================================================================

#[tokio::test]
async fn test_vault_reserve_policy_denied() {
    let (vault_url, _vault_log) = start_mock_vault(Some(403)).await;
    let (provider_url, _provider_log) = start_mock_provider(false).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status.as_u16(), 403);
    assert_eq!(json["error"]["type"], "billing_error");
}

// ============================================================================
// Test d: Vault 429 (rate limited)
// ============================================================================

#[tokio::test]
async fn test_vault_reserve_rate_limited() {
    let (vault_url, _vault_log) = start_mock_vault(Some(429)).await;
    let (provider_url, _provider_log) = start_mock_provider(false).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(status.as_u16(), 429);
    assert_eq!(json["error"]["type"], "billing_error");
}

// ============================================================================
// Test e: Reserve uses frontier rates (BILL-05)
// ============================================================================

#[tokio::test]
async fn test_vault_reserve_uses_frontier_rates() {
    let (vault_url, vault_log) = start_mock_vault(None).await;
    let (provider_url, _provider_log) = start_mock_provider(false).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let _response = app.oneshot(request).await.unwrap();

    // Check the reserve call's amount_msats
    let calls = vault_log.lock().unwrap();
    let reserve_call = calls
        .iter()
        .find(|(op, _, _)| op == "reserve")
        .expect("should have a reserve call");

    let amount_msats = reserve_call.1["amount_msats"].as_u64().unwrap();

    // With frontier rates (input=10, output=30, base=2) and default_reserve_tokens=4096:
    // Input: "hello world test message" = 24 chars / 4 = 6 tokens
    // max_tokens injected = 4096 (default_reserve_tokens)
    // Reserve = (6 * 10 * 1000/1000) + (4096 * 30 * 1000/1000) + (2 * 1000) = 60 + 122880 + 2000 = 124940
    //
    // If local rates (input=1, output=5, base=0) were used instead:
    // Reserve = (6 * 1 * 1000/1000) + (4096 * 5 * 1000/1000) + 0 = 6 + 20480 = 20486
    //
    // Verify frontier rates were used (amount > 100000, not ~20000)
    assert!(
        amount_msats > 100_000,
        "Reserve should use frontier rates (~125k msats), got {} msats. \
         Local rates would produce ~20k msats.",
        amount_msats
    );
}

// ============================================================================
// Test f: Full reserve -> route -> settle path (BILL-02 ordering + BILL-03)
// ============================================================================

#[tokio::test]
async fn test_full_reserve_route_settle_path() {
    let (vault_url, vault_log) = start_mock_vault(None).await;
    let (provider_url, provider_log) = start_mock_provider(false).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    // Response should be a valid chat completion
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(json["model"], "gpt-4o");
    assert_eq!(json["choices"][0]["message"]["content"], "test response");

    // BILL-02: Vault reserve timestamp BEFORE provider contact timestamp
    {
        let vault_calls = vault_log.lock().unwrap();
        let provider_calls = provider_log.lock().unwrap();

        let reserve_call = vault_calls
            .iter()
            .find(|(op, _, _)| op == "reserve")
            .expect("should have a reserve call");
        let provider_call = provider_calls
            .first()
            .expect("provider should have been contacted");

        assert!(
            reserve_call.2 < provider_call.2,
            "BILL-02: vault reserve must happen before provider contact"
        );

        // Verify reserve body contains expected fields
        assert!(
            reserve_call.1["agent_token"].as_str().is_some(),
            "reserve body should contain agent_token"
        );
        assert!(
            reserve_call.1["correlation_id"].as_str().is_some(),
            "reserve body should contain correlation_id"
        );
        assert_eq!(
            reserve_call.1["model"], "gpt-4o",
            "reserve body should contain model"
        );
    }

    // BILL-03: Wait briefly for the spawned settle task, then verify
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    {
        let vault_calls = vault_log.lock().unwrap();
        let settle_call = vault_calls
            .iter()
            .find(|(op, _, _)| op == "settle")
            .expect("BILL-03: settle should be called after successful response");

        assert_eq!(
            settle_call.1["reservation_id"], "res-test-123",
            "settle should reference the reservation_id from reserve"
        );
        assert!(
            settle_call.1["actual_msats"].is_number(),
            "settle should include actual_msats"
        );
        assert!(
            settle_call.1["metadata"].is_object(),
            "settle should include metadata object"
        );
        assert!(
            settle_call.1["metadata"]["provider"].is_string(),
            "settle metadata should include provider name"
        );
        assert!(
            settle_call.1["metadata"]["latency_ms"].is_number(),
            "settle metadata should include latency_ms"
        );
    }
}

// ============================================================================
// Test g: Release on provider failure (BILL-04)
// ============================================================================

#[tokio::test]
async fn test_vault_release_on_provider_failure() {
    let (vault_url, vault_log) = start_mock_vault(None).await;
    // Provider returns 500 for all requests
    let (provider_url, _provider_log) = start_mock_provider(true).await;
    let app = common::setup_vault_test_app(&vault_url, &provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, _json) = common::parse_body(response).await;

    // Provider failed, so we should get an error response
    assert!(
        status.is_server_error() || status.as_u16() == 502,
        "should return error when provider fails, got {}",
        status
    );

    // Wait briefly for the spawned release task
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // BILL-04: Vault release should be called (not settle)
    let vault_calls = vault_log.lock().unwrap();
    let has_release = vault_calls.iter().any(|(op, _, _)| op == "release");
    let has_settle = vault_calls.iter().any(|(op, _, _)| op == "settle");

    assert!(
        has_release,
        "BILL-04: vault release should be called when provider fails"
    );
    assert!(
        !has_settle,
        "BILL-04: vault settle should NOT be called when provider fails"
    );
}

// ============================================================================
// Test h: Free proxy mode (no vault, no auth required) (BILL-08)
// ============================================================================

#[tokio::test]
async fn test_free_proxy_mode_no_vault() {
    let (provider_url, provider_log) = start_mock_provider(false).await;
    let app = common::setup_free_proxy_test_app(&provider_url);

    // Send request WITHOUT Authorization header -- should work in free proxy mode
    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, json) = common::parse_body(response).await;

    assert_eq!(
        status,
        http::StatusCode::OK,
        "free proxy mode should not require auth"
    );
    assert_eq!(json["model"], "gpt-4o");
    assert_eq!(json["choices"][0]["message"]["content"], "test response");

    // Verify provider was actually contacted
    let calls = provider_log.lock().unwrap();
    assert!(
        !calls.is_empty(),
        "provider should be contacted in free proxy mode"
    );
}

// ============================================================================
// Test i: Vault auth replaces server auth (D-01; D-02 superseded)
// ============================================================================

#[tokio::test]
async fn test_vault_auth_replaces_server_auth() {
    let (vault_url, vault_log) = start_mock_vault(None).await;
    let (provider_url, _provider_log) = start_mock_provider(false).await;

    // Create app with BOTH vault config AND server auth_token
    let app = common::setup_vault_test_app_with_auth(
        &vault_url,
        &provider_url,
        Some("server-secret-token"),
    );

    // Send request with a bearer token that does NOT match server auth_token
    // but IS a valid vault agent token. Per D-01, server auth is disabled when
    // vault is configured, so this should NOT be rejected by server auth middleware.
    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer vault-agent-token-not-server-token")
        .body(axum::body::Body::from(chat_request_body()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let (status, _json) = common::parse_body(response).await;

    // Should succeed (200) -- server auth middleware did not reject it
    assert_eq!(
        status,
        http::StatusCode::OK,
        "D-01: vault auth should replace server auth when vault is configured"
    );

    // Verify the request reached the vault reserve endpoint
    let vault_calls = vault_log.lock().unwrap();
    let has_reserve = vault_calls.iter().any(|(op, _, _)| op == "reserve");
    assert!(
        has_reserve,
        "request should reach vault reserve endpoint when vault auth replaces server auth"
    );
}
