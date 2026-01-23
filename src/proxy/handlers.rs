//! HTTP request handlers.

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures::StreamExt;

use super::server::AppState;
use super::types::ChatCompletionRequest;
use crate::error::Error;

/// Custom header for policy selection.
pub const ARBSTR_POLICY_HEADER: &str = "x-arbstr-policy";

/// Handle POST /v1/chat/completions
pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> Result<Response, Error> {
    let policy_name = headers
        .get(ARBSTR_POLICY_HEADER)
        .and_then(|v| v.to_str().ok());

    let user_prompt = request.user_prompt();

    tracing::info!(
        model = %request.model,
        policy = ?policy_name,
        stream = ?request.stream,
        "Received chat completion request"
    );

    // Select provider
    let provider = state
        .router
        .select(&request.model, policy_name, user_prompt)?;

    tracing::info!(
        provider = %provider.name,
        url = %provider.url,
        output_rate = %provider.output_rate,
        "Selected provider"
    );

    // Build upstream URL
    let upstream_url = format!("{}/chat/completions", provider.url.trim_end_matches('/'));

    // Forward request to provider
    let is_streaming = request.stream.unwrap_or(false);

    let mut upstream_request = state
        .http_client
        .post(&upstream_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&request);

    // Add authorization if provider has an API key
    if let Some(api_key) = &provider.api_key {
        upstream_request = upstream_request.header(header::AUTHORIZATION, format!("Bearer {}", api_key));
    }

    let upstream_response = upstream_request.send().await.map_err(|e| {
        tracing::error!(error = %e, provider = %provider.name, "Failed to reach provider");
        Error::Provider(format!("Failed to reach provider '{}': {}", provider.name, e))
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
        return Err(Error::Provider(format!(
            "Provider '{}' returned {}: {}",
            provider.name, status, error_body
        )));
    }

    if is_streaming {
        // Stream response back to client
        let stream = upstream_response.bytes_stream().map(move |chunk| {
            chunk.map_err(|e| {
                tracing::error!(error = %e, "Error streaming from provider");
                std::io::Error::new(std::io::ErrorKind::Other, e)
            })
        });

        let body = Body::from_stream(stream);

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header("x-arbstr-provider", &provider.name)
            .body(body)
            .unwrap())
    } else {
        // Non-streaming: parse and forward response
        let mut response: serde_json::Value = upstream_response.json().await.map_err(|e| {
            tracing::error!(error = %e, "Failed to parse provider response");
            Error::Provider(format!("Failed to parse response from '{}': {}", provider.name, e))
        })?;

        // Add arbstr metadata
        if let Some(obj) = response.as_object_mut() {
            obj.insert(
                "arbstr_provider".to_string(),
                serde_json::Value::String(provider.name.clone()),
            );
        }

        // TODO: Log to database for cost tracking

        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-arbstr-provider", &provider.name)
            .body(Body::from(serde_json::to_vec(&response).unwrap()))
            .unwrap())
    }
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
