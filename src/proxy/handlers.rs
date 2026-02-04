//! HTTP request handlers.

use axum::{
    body::Body,
    extract::{Extension, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures::StreamExt;

use super::server::{AppState, RequestId};
use super::types::ChatCompletionRequest;
use crate::error::Error;
use crate::storage::logging::{spawn_log_write, RequestLog};

/// Custom header for policy selection.
pub const ARBSTR_POLICY_HEADER: &str = "x-arbstr-policy";

/// Response header: correlation ID (UUID v4).
pub const ARBSTR_REQUEST_ID_HEADER: &str = "x-arbstr-request-id";
/// Response header: actual cost in satoshis (decimal, e.g. "42.35").
pub const ARBSTR_COST_SATS_HEADER: &str = "x-arbstr-cost-sats";
/// Response header: wall-clock latency in milliseconds (integer).
pub const ARBSTR_LATENCY_MS_HEADER: &str = "x-arbstr-latency-ms";
/// Response header: provider name that handled the request.
pub const ARBSTR_PROVIDER_HEADER: &str = "x-arbstr-provider";
/// Response header: present with value "true" on streaming responses.
pub const ARBSTR_STREAMING_HEADER: &str = "x-arbstr-streaming";

/// Outcome of a successful request, containing the response and metadata for logging.
struct RequestOutcome {
    response: Response,
    provider_name: String,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cost_sats: Option<f64>,
    provider_cost_sats: Option<f64>,
}

/// Outcome of a failed request, containing the error and metadata for logging.
struct RequestError {
    error: Error,
    provider_name: Option<String>,
    status_code: u16,
    message: String,
}

/// Extract token usage from a provider response.
///
/// Returns (prompt_tokens, completion_tokens) if the usage object is present
/// and contains both fields. Returns None if usage is missing or incomplete.
fn extract_usage(response: &serde_json::Value) -> Option<(u32, u32)> {
    let usage = response.get("usage")?;
    let input = usage.get("prompt_tokens")?.as_u64()? as u32;
    let output = usage.get("completion_tokens")?.as_u64()? as u32;
    Some((input, output))
}

/// Attach arbstr metadata headers to a response.
///
/// For non-streaming responses: sets request-id, latency, provider, and cost.
/// For streaming responses: sets request-id, provider, and streaming flag.
/// Cost and latency are omitted on streaming responses (not known at header-send time).
fn attach_arbstr_headers(
    response: &mut Response,
    request_id: &str,
    latency_ms: i64,
    provider: Option<&str>,
    cost_sats: Option<f64>,
    is_streaming: bool,
) {
    let headers = response.headers_mut();

    // Always present
    headers.insert(
        HeaderName::from_static(ARBSTR_REQUEST_ID_HEADER),
        HeaderValue::from_str(request_id).unwrap(),
    );

    if is_streaming {
        headers.insert(
            HeaderName::from_static(ARBSTR_STREAMING_HEADER),
            HeaderValue::from_static("true"),
        );
        // Streaming: omit cost and latency (not known at header-send time)
    } else {
        // Non-streaming: always include latency
        headers.insert(
            HeaderName::from_static(ARBSTR_LATENCY_MS_HEADER),
            HeaderValue::from(latency_ms as u64),
        );
        // Non-streaming: include cost if known
        if let Some(cost) = cost_sats {
            headers.insert(
                HeaderName::from_static(ARBSTR_COST_SATS_HEADER),
                HeaderValue::from_str(&format!("{:.2}", cost)).unwrap(),
            );
        }
    }

    // Provider: present when known
    if let Some(provider_name) = provider {
        headers.insert(
            HeaderName::from_static(ARBSTR_PROVIDER_HEADER),
            HeaderValue::from_str(provider_name).unwrap(),
        );
    }
}

/// Handle POST /v1/chat/completions
pub async fn chat_completions(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Response, Error> {
    let start = std::time::Instant::now();
    let correlation_id = request_id.0.to_string();
    let model = request.model.clone();
    let is_streaming = request.stream.unwrap_or(false);

    let policy_name = headers
        .get(ARBSTR_POLICY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let user_prompt = request.user_prompt();

    tracing::info!(
        model = %request.model,
        policy = ?policy_name,
        stream = ?request.stream,
        "Received chat completion request"
    );

    // Execute the core request logic, capturing the outcome for logging
    let result = execute_request(
        &state,
        &request,
        policy_name.as_deref(),
        user_prompt,
        is_streaming,
    )
    .await;

    // Log the outcome (fire-and-forget)
    let latency_ms = start.elapsed().as_millis() as i64;
    if let Some(pool) = &state.db {
        let log_entry = match &result {
            Ok(outcome) => RequestLog {
                correlation_id: correlation_id.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                model: model.clone(),
                provider: Some(outcome.provider_name.clone()),
                policy: policy_name.clone(),
                streaming: is_streaming,
                input_tokens: outcome.input_tokens,
                output_tokens: outcome.output_tokens,
                cost_sats: outcome.cost_sats,
                provider_cost_sats: outcome.provider_cost_sats,
                latency_ms,
                success: true,
                error_status: None,
                error_message: None,
            },
            Err(outcome_err) => RequestLog {
                correlation_id: correlation_id.clone(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                model: model.clone(),
                provider: outcome_err.provider_name.clone(),
                policy: policy_name.clone(),
                streaming: is_streaming,
                input_tokens: None,
                output_tokens: None,
                cost_sats: None,
                provider_cost_sats: None,
                latency_ms,
                success: false,
                error_status: Some(outcome_err.status_code),
                error_message: Some(outcome_err.message.clone()),
            },
        };
        spawn_log_write(pool, log_entry);
    }

    // Convert outcome to HTTP response, attaching arbstr metadata headers
    match result {
        Ok(outcome) => {
            let mut response = outcome.response;
            attach_arbstr_headers(
                &mut response,
                &correlation_id,
                latency_ms,
                Some(&outcome.provider_name),
                outcome.cost_sats,
                is_streaming,
            );
            Ok(response)
        }
        Err(outcome_err) => {
            let mut error_response = outcome_err.error.into_response();
            attach_arbstr_headers(
                &mut error_response,
                &correlation_id,
                latency_ms,
                outcome_err.provider_name.as_deref(),
                None, // cost not known for errors
                is_streaming,
            );
            Ok(error_response)
        }
    }
}

/// Execute the core request logic (provider selection, forwarding, response handling).
///
/// Returns Ok(RequestOutcome) on success or Err(RequestError) on any failure.
/// This separation allows the caller to log both outcomes before returning.
async fn execute_request(
    state: &AppState,
    request: &ChatCompletionRequest,
    policy_name: Option<&str>,
    user_prompt: Option<&str>,
    is_streaming: bool,
) -> std::result::Result<RequestOutcome, RequestError> {
    // Select provider
    let provider = state
        .router
        .select(&request.model, policy_name, user_prompt)
        .map_err(|e| {
            let (status_code, message) = match &e {
                Error::NoProviders { .. } => (400u16, e.to_string()),
                Error::NoPolicyMatch => (400, e.to_string()),
                Error::BadRequest(_) => (400, e.to_string()),
                _ => (500, e.to_string()),
            };
            RequestError {
                error: e,
                provider_name: None,
                status_code,
                message,
            }
        })?;

    tracing::info!(
        provider = %provider.name,
        url = %provider.url,
        output_rate = %provider.output_rate,
        "Selected provider"
    );

    // Build upstream URL
    let upstream_url = format!("{}/chat/completions", provider.url.trim_end_matches('/'));

    // Forward request to provider
    let mut upstream_request = state
        .http_client
        .post(&upstream_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(request);

    if let Some(api_key) = &provider.api_key {
        upstream_request =
            upstream_request.header(header::AUTHORIZATION, format!("Bearer {}", api_key));
    }

    let upstream_response = upstream_request.send().await.map_err(|e| {
        tracing::error!(error = %e, provider = %provider.name, "Failed to reach provider");
        RequestError {
            error: Error::Provider(format!(
                "Failed to reach provider '{}': {}",
                provider.name, e
            )),
            provider_name: Some(provider.name.clone()),
            status_code: 502,
            message: format!("Failed to reach provider: {}", e),
        }
    })?;

    let status = upstream_response.status();
    if !status.is_success() {
        let error_body = upstream_response.text().await.unwrap_or_default();
        tracing::error!(
            status = %status,
            provider = %provider.name,
            body = %error_body,
            "Provider returned error"
        );
        return Err(RequestError {
            error: Error::Provider(format!(
                "Provider '{}' returned {}: {}",
                provider.name, status, error_body
            )),
            provider_name: Some(provider.name.clone()),
            status_code: status.as_u16(),
            message: format!("Provider returned {}", status),
        });
    }

    if is_streaming {
        handle_streaming_response(upstream_response, &provider).await
    } else {
        handle_non_streaming_response(upstream_response, &provider).await
    }
}

/// Handle a non-streaming provider response.
///
/// Extracts the usage object for token counts and calculates cost.
async fn handle_non_streaming_response(
    upstream_response: reqwest::Response,
    provider: &crate::router::SelectedProvider,
) -> std::result::Result<RequestOutcome, RequestError> {
    let mut response: serde_json::Value = upstream_response.json().await.map_err(|e| {
        tracing::error!(error = %e, "Failed to parse provider response");
        RequestError {
            error: Error::Provider(format!(
                "Failed to parse response from '{}': {}",
                provider.name, e
            )),
            provider_name: Some(provider.name.clone()),
            status_code: 502,
            message: format!("Failed to parse response: {}", e),
        }
    })?;

    // Extract usage for logging
    let usage = extract_usage(&response);
    let (input_tokens, output_tokens) = match usage {
        Some((input, output)) => (Some(input), Some(output)),
        None => (None, None),
    };

    // Calculate arbstr cost using config rates
    let cost_sats = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => Some(crate::router::actual_cost_sats(
            input,
            output,
            provider.input_rate,
            provider.output_rate,
            provider.base_fee,
        )),
        _ => None,
    };

    // Extract provider-reported cost (if present in response)
    let provider_cost_sats = response
        .get("usage")
        .and_then(|u| u.get("total_cost"))
        .and_then(|v| v.as_f64());

    // Add arbstr metadata to response
    if let Some(obj) = response.as_object_mut() {
        obj.insert(
            "arbstr_provider".to_string(),
            serde_json::Value::String(provider.name.clone()),
        );
    }

    let http_response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&response).unwrap()))
        .unwrap();

    Ok(RequestOutcome {
        response: http_response,
        provider_name: provider.name.clone(),
        input_tokens,
        output_tokens,
        cost_sats,
        provider_cost_sats,
    })
}

/// Handle a streaming provider response.
///
/// Passes SSE chunks through to the client. For Phase 2, streaming requests
/// are logged with None tokens/cost because the stream has not been consumed
/// at the point we return the response. Full streaming usage tracking would
/// require wrapping the stream body to detect end-of-stream, which is deferred
/// to a future enhancement.
async fn handle_streaming_response(
    upstream_response: reqwest::Response,
    provider: &crate::router::SelectedProvider,
) -> std::result::Result<RequestOutcome, RequestError> {
    let provider_name = provider.name.clone();

    let stream = upstream_response.bytes_stream().map(move |chunk| {
        match chunk {
            Ok(ref bytes) => {
                // Try to extract usage from SSE data lines in this chunk
                // (for future reference -- usage may appear in final chunk)
                if let Ok(text) = std::str::from_utf8(bytes) {
                    for line in text.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data != "[DONE]" {
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(data)
                                {
                                    if let Some(usage) =
                                        parsed.get("usage").filter(|u| !u.is_null())
                                    {
                                        if let (Some(input), Some(output)) = (
                                            usage
                                                .get("prompt_tokens")
                                                .and_then(|v| v.as_u64()),
                                            usage
                                                .get("completion_tokens")
                                                .and_then(|v| v.as_u64()),
                                        ) {
                                            tracing::debug!(
                                                input_tokens = input,
                                                output_tokens = output,
                                                "Captured usage from streaming chunk"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Err(ref e) => {
                tracing::error!(error = %e, "Error streaming from provider");
            }
        }
        chunk.map_err(std::io::Error::other)
    });

    let body = Body::from_stream(stream);

    let http_response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap();

    // For streaming, we return the response immediately.
    // Usage data is not yet available because the stream hasn't been consumed.
    // Per CONTEXT.md: "if missing or incomplete, log with null token/cost fields"
    Ok(RequestOutcome {
        response: http_response,
        provider_name,
        input_tokens: None,
        output_tokens: None,
        cost_sats: None,
        provider_cost_sats: None,
    })
}

/// Handle GET /v1/models - list available models across all providers
pub async fn list_models(State(state): State<AppState>) -> impl IntoResponse {
    let mut models: Vec<serde_json::Value> = vec![];
    let mut seen = std::collections::HashSet::new();

    for provider in state.router.providers() {
        for model in &provider.models {
            if seen.insert(model.clone()) {
                models.push(serde_json::json!({
                    "id": model,
                    "object": "model",
                    "owned_by": "routstr",
                }));
            }
        }
    }

    Json(serde_json::json!({
        "object": "list",
        "data": models
    }))
}

/// Handle GET /health
pub async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "status": "ok",
        "service": "arbstr"
    }))
}

/// Handle GET /providers - arbstr extension to list providers
pub async fn list_providers(State(state): State<AppState>) -> impl IntoResponse {
    let providers: Vec<serde_json::Value> = state
        .router
        .providers()
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "models": p.models,
                "input_rate_sats_per_1k": p.input_rate,
                "output_rate_sats_per_1k": p.output_rate,
                "base_fee_sats": p.base_fee,
            })
        })
        .collect();

    Json(serde_json::json!({
        "providers": providers
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_usage_present() {
        let response = serde_json::json!({
            "id": "chatcmpl-123",
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 200,
                "total_tokens": 300
            }
        });
        let usage = extract_usage(&response);
        assert_eq!(usage, Some((100, 200)));
    }

    #[test]
    fn test_extract_usage_missing() {
        let response = serde_json::json!({
            "id": "chatcmpl-123",
            "choices": []
        });
        let usage = extract_usage(&response);
        assert_eq!(usage, None);
    }

    #[test]
    fn test_extract_usage_partial() {
        let response = serde_json::json!({
            "id": "chatcmpl-123",
            "choices": [],
            "usage": {
                "prompt_tokens": 100
            }
        });
        let usage = extract_usage(&response);
        assert_eq!(usage, None);
    }

    #[test]
    fn test_extract_usage_null() {
        let response = serde_json::json!({
            "id": "chatcmpl-123",
            "choices": [],
            "usage": null
        });
        let usage = extract_usage(&response);
        assert_eq!(usage, None);
    }

    #[test]
    fn test_attach_headers_non_streaming() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();
        attach_arbstr_headers(
            &mut response,
            "550e8400-e29b-41d4-a716-446655440000",
            1523,
            Some("provider-alpha"),
            Some(42.35),
            false,
        );
        let headers = response.headers();
        assert_eq!(
            headers.get("x-arbstr-request-id").unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(headers.get("x-arbstr-latency-ms").unwrap(), "1523");
        assert_eq!(headers.get("x-arbstr-cost-sats").unwrap(), "42.35");
        assert_eq!(headers.get("x-arbstr-provider").unwrap(), "provider-alpha");
        assert!(headers.get("x-arbstr-streaming").is_none());
    }

    #[test]
    fn test_attach_headers_streaming() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();
        attach_arbstr_headers(
            &mut response,
            "550e8400-e29b-41d4-a716-446655440000",
            500,
            Some("provider-beta"),
            Some(10.00), // cost provided but should be ignored for streaming
            true,
        );
        let headers = response.headers();
        assert_eq!(
            headers.get("x-arbstr-request-id").unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(headers.get("x-arbstr-streaming").unwrap(), "true");
        assert_eq!(headers.get("x-arbstr-provider").unwrap(), "provider-beta");
        // Streaming omits cost and latency
        assert!(headers.get("x-arbstr-cost-sats").is_none());
        assert!(headers.get("x-arbstr-latency-ms").is_none());
    }

    #[test]
    fn test_attach_headers_error_no_provider() {
        let mut response = Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .unwrap();
        attach_arbstr_headers(
            &mut response,
            "abcd1234-0000-0000-0000-000000000000",
            50,
            None,  // no provider (pre-route error)
            None,  // no cost
            false,
        );
        let headers = response.headers();
        assert_eq!(
            headers.get("x-arbstr-request-id").unwrap(),
            "abcd1234-0000-0000-0000-000000000000"
        );
        assert_eq!(headers.get("x-arbstr-latency-ms").unwrap(), "50");
        assert!(headers.get("x-arbstr-provider").is_none());
        assert!(headers.get("x-arbstr-cost-sats").is_none());
        assert!(headers.get("x-arbstr-streaming").is_none());
    }

    #[test]
    fn test_attach_headers_no_cost() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();
        attach_arbstr_headers(
            &mut response,
            "11111111-2222-3333-4444-555555555555",
            200,
            Some("provider-gamma"),
            None, // cost unknown
            false,
        );
        let headers = response.headers();
        assert_eq!(
            headers.get("x-arbstr-request-id").unwrap(),
            "11111111-2222-3333-4444-555555555555"
        );
        assert_eq!(headers.get("x-arbstr-latency-ms").unwrap(), "200");
        assert_eq!(headers.get("x-arbstr-provider").unwrap(), "provider-gamma");
        assert!(headers.get("x-arbstr-cost-sats").is_none());
    }

    #[test]
    fn test_attach_headers_cost_formatting() {
        let mut response = Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap();
        attach_arbstr_headers(
            &mut response,
            "00000000-0000-0000-0000-000000000000",
            100,
            Some("provider"),
            Some(0.10), // should format as "0.10" not "0.1"
            false,
        );
        assert_eq!(
            response.headers().get("x-arbstr-cost-sats").unwrap(),
            "0.10"
        );
    }
}
