//! Integration tests for stream_options injection.
//!
//! Verifies that arbstr injects `stream_options: { include_usage: true }` into
//! upstream request bodies for streaming requests, and omits it for non-streaming.

use arbstr::proxy::types::{ensure_stream_options, ChatCompletionRequest, Message, StreamOptions};

/// Streaming request gets stream_options injected when absent.
#[test]
fn streaming_request_gets_stream_options_injected() {
    let mut request = ChatCompletionRequest {
        model: "gpt-4o".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: "Write a poem".to_string(),
            name: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        stream_options: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
    };

    ensure_stream_options(&mut request);

    let json = serde_json::to_string(&request).unwrap();
    assert!(
        json.contains(r#""stream_options":{"include_usage":true}"#),
        "Upstream body should contain stream_options with include_usage:true: {}",
        json
    );
}

/// Non-streaming request does NOT get stream_options when absent.
#[test]
fn non_streaming_request_has_no_stream_options() {
    let request = ChatCompletionRequest {
        model: "gpt-4o".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: "Write a poem".to_string(),
            name: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: Some(false),
        stream_options: None,
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
    };

    // Non-streaming: ensure_stream_options is NOT called, so field stays None
    let json = serde_json::to_string(&request).unwrap();
    assert!(
        !json.contains("stream_options"),
        "Non-streaming body should NOT contain stream_options: {}",
        json
    );
}

/// Client-provided stream_options are preserved (merge, not overwrite).
#[test]
fn client_stream_options_preserved_on_merge() {
    let mut request = ChatCompletionRequest {
        model: "gpt-4o".to_string(),
        messages: vec![Message {
            role: "user".to_string(),
            content: "Hello".to_string(),
            name: None,
        }],
        temperature: None,
        max_tokens: None,
        stream: Some(true),
        stream_options: Some(StreamOptions {
            include_usage: Some(false),
        }),
        top_p: None,
        frequency_penalty: None,
        presence_penalty: None,
        stop: None,
        user: None,
    };

    ensure_stream_options(&mut request);

    // Client explicitly set include_usage to false -- merge preserves it
    let opts = request.stream_options.as_ref().unwrap();
    assert_eq!(opts.include_usage, Some(false));
}

/// Round-trip JSON deserialization preserves stream_options.
#[test]
fn stream_options_roundtrip_json() {
    let json_input = r#"{
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "hi"}],
        "stream": true,
        "stream_options": {"include_usage": true}
    }"#;

    let request: ChatCompletionRequest = serde_json::from_str(json_input).unwrap();
    assert!(request.stream_options.is_some());
    assert_eq!(request.stream_options.as_ref().unwrap().include_usage, Some(true));

    // Re-serialize and verify field is present
    let json_output = serde_json::to_string(&request).unwrap();
    assert!(json_output.contains(r#""stream_options":{"include_usage":true}"#));
}
