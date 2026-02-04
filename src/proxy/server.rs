//! HTTP server setup and configuration.

use axum::{
    middleware,
    response::Response,
    routing::{get, post},
    Router,
};
use reqwest::Client;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use super::handlers;
use crate::config::Config;
use crate::router::Router as ProviderRouter;

/// Per-request correlation ID stored in request extensions.
#[derive(Clone, Debug)]
pub struct RequestId(pub Uuid);

/// Shared application state.
#[derive(Clone)]
pub struct AppState {
    pub router: Arc<ProviderRouter>,
    pub http_client: Client,
    pub config: Arc<Config>,
    pub db: Option<SqlitePool>,
}

/// Middleware that generates a correlation ID and stores it in request extensions.
async fn inject_request_id(
    mut request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    let request_id = Uuid::new_v4();
    request.extensions_mut().insert(RequestId(request_id));
    next.run(request).await
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
        .layer(
            TraceLayer::new_for_http().make_span_with(
                |request: &axum::http::Request<axum::body::Body>| {
                    let request_id = request
                        .extensions()
                        .get::<RequestId>()
                        .map(|r| r.0)
                        .unwrap_or_else(Uuid::new_v4);
                    tracing::info_span!(
                        "request",
                        method = %request.method(),
                        uri = %request.uri(),
                        request_id = %request_id,
                    )
                },
            ),
        )
        .layer(middleware::from_fn(inject_request_id))
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

    // Initialize database pool if configured
    let db = {
        let db_config = config.database();
        match crate::storage::init_pool(&db_config.path).await {
            Ok(pool) => {
                tracing::info!(path = %db_config.path, "Database initialized");
                Some(pool)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize database, logging disabled");
                None
            }
        }
    };

    let state = AppState {
        router: Arc::new(provider_router),
        http_client,
        config: Arc::new(config),
        db,
    };

    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!(address = %listen_addr, "Starting arbstr proxy server");

    axum::serve(listener, app).await?;

    Ok(())
}
