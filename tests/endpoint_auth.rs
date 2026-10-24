//! Authentication on the analytics and cost-free proxy endpoints.
//!
//! - /v1/stats, /v1/requests: operator_token; in free proxy mode auth_token is the
//!   fallback and no token means open; in vault mode no operator_token means refused
//! - /v1/models, /v1/cost: auth_token in free proxy mode; in vault mode a
//!   vault-verified agent token (POST /internal/agents/verify, nothing reserved)
//! - /health and /providers stay open

mod common;

use std::sync::{Arc, Mutex};

use axum::routing::post;
use axum::Router;
use http::{Request, StatusCode};
use tower::ServiceExt;

const AGENT_TOKEN: &str = "vtk_good_agent";
const OPS_TOKEN: &str = "ops-token-for-tests";
const AUTH_TOKEN: &str = "proxy-auth-token";

/// Mock vault: /internal/agents/verify accepts AGENT_TOKEN, answers `status_override`
/// when set, and records each verified token. /internal/reserve must never be hit.
async fn start_mock_vault(status_override: Option<u16>) -> (String, Arc<Mutex<Vec<String>>>) {
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let verify_calls = calls.clone();
    let reserve_calls = calls.clone();

    let app = Router::new()
        .route(
            "/internal/agents/verify",
            post(
                move |headers: axum::http::HeaderMap, body: axum::Json<serde_json::Value>| {
                    let calls = verify_calls.clone();
                    async move {
                        assert_eq!(
                            headers.get("X-Internal-Token").and_then(|v| v.to_str().ok()),
                            Some("test-internal-token")
                        );
                        let token = body.0["agent_token"].as_str().unwrap_or("").to_string();
                        calls.lock().unwrap().push(token.clone());
                        if let Some(code) = status_override {
                            return (
                                StatusCode::from_u16(code).unwrap(),
                                axum::Json(serde_json::json!({"error": {"code": "x", "message": "x"}})),
                            );
                        }
                        if token == AGENT_TOKEN {
                            (StatusCode::OK, axum::Json(serde_json::json!({"agent_id": "ag_good"})))
                        } else {
                            (
                                StatusCode::UNAUTHORIZED,
                                axum::Json(serde_json::json!({
                                    "error": {"code": "invalid_agent_token", "message": "Invalid agent token"}
                                })),
                            )
                        }
                    }
                },
            ),
        )
        .route(
            "/internal/reserve",
            post(move || {
                let calls = reserve_calls.clone();
                async move {
                    calls.lock().unwrap().push("RESERVE".to_string());
                    (StatusCode::INTERNAL_SERVER_ERROR, "reserve must not be called")
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), calls)
}

fn get(path: &str, token: Option<&str>) -> Request<axum::body::Body> {
    let mut req = Request::get(path);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {}", t));
    }
    req.body(axum::body::Body::empty()).unwrap()
}

fn cost(token: Option<&str>) -> Request<axum::body::Body> {
    let mut req = Request::post("/v1/cost").header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {}", t));
    }
    req.body(axum::body::Body::from(
        r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#,
    ))
    .unwrap()
}

async fn status(app: &Router, req: Request<axum::body::Body>) -> (StatusCode, serde_json::Value) {
    common::parse_body(app.clone().oneshot(req).await.unwrap()).await
}

// ── Analytics endpoints ─────────────────────────────────────────────────────

#[tokio::test]
async fn vault_mode_without_operator_token_refuses_analytics() {
    let (vault_url, _) = start_mock_vault(None).await;
    let app = common::setup_vault_test_app(&vault_url, "http://127.0.0.1:1");

    for path in ["/v1/stats", "/v1/requests"] {
        for token in [None, Some(AGENT_TOKEN), Some("anything")] {
            let (s, body) = status(&app, get(path, token)).await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "{} {:?}", path, token);
            assert_eq!(body["error"]["code"], "operator_token_not_configured");
            assert!(body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("operator_token"));
        }
    }
}

#[tokio::test]
async fn vault_mode_operator_token_guards_analytics() {
    let (vault_url, calls) = start_mock_vault(None).await;
    let app = common::setup_vault_test_app_with_tokens(
        &vault_url,
        "http://127.0.0.1:1",
        None,
        Some(OPS_TOKEN),
    );

    for path in ["/v1/stats", "/v1/requests"] {
        assert_eq!(
            status(&app, get(path, None)).await.0,
            StatusCode::UNAUTHORIZED
        );
        // An agent token is not an operator credential
        let (s, body) = status(&app, get(path, Some(AGENT_TOKEN))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "invalid_api_key");
        // The operator token gets past auth (no DB in this harness, so not 200)
        assert_ne!(
            status(&app, get(path, Some(OPS_TOKEN))).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(
        calls.lock().unwrap().is_empty(),
        "analytics must not call vault"
    );
}

#[tokio::test]
async fn free_mode_analytics_use_operator_token_then_auth_token_then_open() {
    let upstream = "http://127.0.0.1:1";

    // No tokens: open (local free proxy)
    let app = common::setup_free_proxy_test_app_with_tokens(upstream, None, None);
    assert_ne!(
        status(&app, get("/v1/stats", None)).await.0,
        StatusCode::UNAUTHORIZED
    );

    // auth_token only: it guards analytics too
    let app = common::setup_free_proxy_test_app_with_tokens(upstream, Some(AUTH_TOKEN), None);
    assert_eq!(
        status(&app, get("/v1/requests", None)).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_ne!(
        status(&app, get("/v1/requests", Some(AUTH_TOKEN))).await.0,
        StatusCode::UNAUTHORIZED
    );

    // operator_token set: it wins over auth_token for analytics
    let app =
        common::setup_free_proxy_test_app_with_tokens(upstream, Some(AUTH_TOKEN), Some(OPS_TOKEN));
    assert_eq!(
        status(&app, get("/v1/stats", Some(AUTH_TOKEN))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_ne!(
        status(&app, get("/v1/stats", Some(OPS_TOKEN))).await.0,
        StatusCode::UNAUTHORIZED
    );
    // ...and auth_token still guards the proxy endpoints
    assert_eq!(
        status(&app, get("/v1/models", Some(OPS_TOKEN))).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&app, get("/v1/models", Some(AUTH_TOKEN))).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn wrong_token_of_the_same_length_is_rejected() {
    let app =
        common::setup_free_proxy_test_app_with_tokens("http://127.0.0.1:1", None, Some(OPS_TOKEN));
    let wrong: String = OPS_TOKEN.chars().rev().collect();
    assert_eq!(wrong.len(), OPS_TOKEN.len());
    assert_eq!(
        status(&app, get("/v1/stats", Some(&wrong))).await.0,
        StatusCode::UNAUTHORIZED
    );
}

// ── Cost-free proxy endpoints in vault mode ─────────────────────────────────

#[tokio::test]
async fn vault_mode_models_and_cost_require_a_verified_agent_token() {
    let (vault_url, calls) = start_mock_vault(None).await;
    let app = common::setup_vault_test_app(&vault_url, "http://127.0.0.1:1");

    // Missing token: refused before vault is asked
    let (s, body) = status(&app, get("/v1/models", None)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "invalid_api_key");
    assert!(calls.lock().unwrap().is_empty());

    // Unknown agent: vault says invalid_agent_token
    let (s, body) = status(&app, get("/v1/models", Some("vtk_nope"))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], "vault_401");

    // Known agent: both endpoints work
    let (s, body) = status(&app, get("/v1/models", Some(AGENT_TOKEN))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["object"], "list");
    let (s, body) = status(&app, cost(Some(AGENT_TOKEN))).await;
    assert_eq!(s, StatusCode::OK, "{}", body);

    let calls = calls.lock().unwrap();
    assert_eq!(
        *calls,
        vec!["vtk_nope", AGENT_TOKEN, AGENT_TOKEN],
        "verified, never reserved"
    );
}

#[tokio::test]
async fn vault_mode_verify_failures_map_to_vault_status() {
    // Vault down → 503; vault too old to have the endpoint → 500 naming the upgrade
    for (code, expected) in [
        (503, StatusCode::SERVICE_UNAVAILABLE),
        (404, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        let (vault_url, _) = start_mock_vault(Some(code)).await;
        let app = common::setup_vault_test_app(&vault_url, "http://127.0.0.1:1");
        let (s, body) = status(&app, get("/v1/models", Some(AGENT_TOKEN))).await;
        assert_eq!(s, expected, "vault {}", code);
        assert_eq!(body["error"]["type"], "billing_error");
        if code == 404 {
            assert!(body["error"]["message"]
                .as_str()
                .unwrap()
                .contains("upgrade arbstr-vault"));
        }
    }
}

#[tokio::test]
async fn health_and_providers_stay_open() {
    let (vault_url, _) = start_mock_vault(None).await;
    let app = common::setup_vault_test_app_with_tokens(
        &vault_url,
        "http://127.0.0.1:1",
        None,
        Some(OPS_TOKEN),
    );
    for path in ["/health", "/providers"] {
        assert_ne!(
            status(&app, get(path, None)).await.0,
            StatusCode::UNAUTHORIZED,
            "{}",
            path
        );
    }
}
