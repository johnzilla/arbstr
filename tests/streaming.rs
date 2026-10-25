//! Streaming responses through the router: the client sees the provider's
//! events, then arbstr's trailing cost event, then exactly one `[DONE]`.

mod common;

use axum::http::Request;
use axum::routing::post;
use tower::ServiceExt;

async fn start_streaming_provider() -> String {
    let body = concat!(
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}\n\n",
        "data: [DONE]\n\n",
    );
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .body(axum::body::Body::from(body))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{}", addr)
}

#[tokio::test]
async fn stream_ends_with_cost_event_and_a_single_done() {
    let provider_url = start_streaming_provider().await;
    let app = common::setup_free_proxy_test_app(&provider_url);

    let request = Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({
                "model": "gpt-4o",
                "stream": true,
                "messages": [{"role": "user", "content": "hello"}]
            })
            .to_string(),
        ))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    assert_eq!(text.matches("[DONE]").count(), 1, "stream: {text}");
    assert!(text.trim_end().ends_with("data: [DONE]"), "stream: {text}");
    let cost_event = text.find("\"arbstr\"").expect("trailing arbstr event");
    assert!(cost_event < text.find("[DONE]").unwrap());
    assert!(
        text.contains("\"content\":\"hi\""),
        "provider events forwarded"
    );
}
