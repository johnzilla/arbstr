//! Per-client rate limiting (`[server] client_rate_limit_rps`) through the router.

mod common;

use axum::http::Request;
use tower::ServiceExt;

fn models(token: Option<&str>) -> Request<axum::body::Body> {
    let mut builder = Request::get("/v1/models");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder.body(axum::body::Body::empty()).unwrap()
}

#[tokio::test]
async fn each_client_gets_its_own_budget() {
    let app = common::setup_free_proxy_test_app_with_client_limit("http://127.0.0.1:9", 2);

    for _ in 0..2 {
        let res = app.clone().oneshot(models(Some("agent-a"))).await.unwrap();
        assert_eq!(res.status(), http::StatusCode::OK);
    }
    let res = app.clone().oneshot(models(Some("agent-a"))).await.unwrap();
    assert_eq!(res.status(), http::StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(res.headers()["retry-after"], "1");
    let (_, body) = common::parse_body(res).await;
    assert_eq!(body["error"]["code"], "client_rate_limited");

    // Another token is unaffected
    let res = app.clone().oneshot(models(Some("agent-b"))).await.unwrap();
    assert_eq!(res.status(), http::StatusCode::OK);
}

#[tokio::test]
async fn health_is_exempt() {
    let app = common::setup_free_proxy_test_app_with_client_limit("http://127.0.0.1:9", 1);
    for _ in 0..5 {
        let req = Request::get("/health")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), http::StatusCode::OK);
    }
}

#[tokio::test]
async fn off_by_default() {
    let app = common::setup_free_proxy_test_app("http://127.0.0.1:9");
    for _ in 0..20 {
        let res = app.clone().oneshot(models(None)).await.unwrap();
        assert_eq!(res.status(), http::StatusCode::OK);
    }
}
