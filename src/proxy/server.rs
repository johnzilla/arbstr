//! HTTP server setup and configuration.

use axum::{
    routing::{get, post},
    Router,
};
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tower_http::trace::TraceLayer;

use super::handlers;
use crate::config::Config;
use crate::router::Router as ProviderRouter;

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
}

/// Create the axum router with all endpoints.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // OpenAI-compatible endpoints
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/models", get(handlers::list_models))
        // arbstr extensions
        .route("/health", get(handlers::health))
        .route("/providers", get(handlers::list_providers))
        // State and middleware
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// Run the HTTP server.
pub async fn run_server(config: Config) -> anyhow::Result<()> {
    let listen_addr = config.server.listen.clone();

    // Create provider router
    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );

    // Create HTTP client with reasonable defaults
    let http_client = Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    let state = AppState {
        router: Arc::new(provider_router),
        http_client,
        config: Arc::new(config),
    };

    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!(address = %listen_addr, "Starting arbstr proxy server");

    axum::serve(listener, app).await?;

    Ok(())
}
