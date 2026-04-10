//! Model discovery for providers with OpenAI-compatible /v1/models endpoints.
//!
//! Called once during server startup. Providers with `auto_discover = true`
//! have their static `models` list replaced with the discovered model IDs.

use crate::config::ProviderConfig;
use reqwest::Client;
use std::time::Duration;

#[derive(serde::Deserialize)]
struct ModelsResponse {
    data: Vec<ModelEntry>,
}

#[derive(serde::Deserialize)]
struct ModelEntry {
    id: String,
}

/// Discover models for providers with auto_discover enabled.
/// Called once during server startup (no periodic refresh).
/// On success, replaces provider.models with discovered ids (exact names from endpoint).
/// On failure, logs warning and keeps static models (non-blocking startup).
pub async fn discover_models(providers: &mut [ProviderConfig], client: &Client) {
    for provider in providers.iter_mut() {
        if !provider.auto_discover {
            continue;
        }

        let url = format!("{}/models", provider.url.trim_end_matches('/'));
        tracing::info!(provider = %provider.name, url = %url, "Discovering models");

        match client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<ModelsResponse>().await {
                    Ok(models_resp) => {
                        let model_ids: Vec<String> =
                            models_resp.data.into_iter().map(|m| m.id).collect();
                        tracing::info!(
                            provider = %provider.name,
                            models = ?model_ids,
                            count = model_ids.len(),
                            "Discovered models"
                        );
                        provider.models = model_ids;
                    }
                    Err(e) => {
                        tracing::warn!(
                            provider = %provider.name,
                            error = %e,
                            "Failed to parse /v1/models response, keeping static models"
                        );
                    }
                }
            }
            Ok(resp) => {
                tracing::warn!(
                    provider = %provider.name,
                    status = %resp.status(),
                    "Discovery endpoint returned non-success status, keeping static models"
                );
            }
            Err(e) => {
                tracing::warn!(
                    provider = %provider.name,
                    error = %e,
                    "Failed to reach discovery endpoint, keeping static models"
                );
                if provider.models.is_empty() {
                    tracing::warn!(
                        provider = %provider.name,
                        "Provider has no models after failed discovery -- it won't match any requests until restarted with the provider available"
                    );
                }
            }
        }
    }
}
