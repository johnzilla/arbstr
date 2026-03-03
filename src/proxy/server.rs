//! HTTP server setup and configuration.

use axum::{
    error_handling::HandleErrorLayer,
    middleware,
    response::Response,
    routing::{get, post},
    Router,
};
use reqwest::Client;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use super::circuit_breaker::CircuitBreakerRegistry;
use super::handlers;
use crate::config::Config;
use crate::router::Router as ProviderRouter;
use crate::storage::DbWriter;

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
    pub read_db: Option<SqlitePool>,
    pub db_writer: Option<DbWriter>,
    pub circuit_breakers: Arc<CircuitBreakerRegistry>,
}

/// Middleware that verifies the `Authorization: Bearer <token>` header.
///
/// Returns 401 Unauthorized if the token is missing or incorrect.
/// Only applied when `server.auth_token` is set in config.
async fn auth_middleware(
    expected_token: Arc<String>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.strip_prefix("Bearer ") == Some(expected_token.as_str()) => {
            next.run(request).await
        }
        _ => {
            let body = serde_json::json!({
                "error": {
                    "message": "Invalid or missing bearer token",
                    "type": "authentication_error",
                    "code": "invalid_api_key"
                }
            });
            Response::builder()
                .status(axum::http::StatusCode::UNAUTHORIZED)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap()
        }
    }
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
    let rate_limit_rps = state.config.server.rate_limit_rps;
    let auth_token = state.config.server.auth_token.clone();

    // Proxy endpoints that require auth (when configured)
    let proxy_routes = Router::new()
        .route("/v1/chat/completions", post(handlers::chat_completions))
        .route("/v1/models", get(handlers::list_models))
        .route("/v1/cost", post(handlers::cost_estimate));

    // Apply auth middleware only if a token is configured
    let proxy_routes = if let Some(token) = auth_token {
        let token = Arc::new(token);
        proxy_routes.layer(middleware::from_fn(move |req, next| {
            let token = token.clone();
            auth_middleware(token, req, next)
        }))
    } else {
        proxy_routes
    };

    let mut app = proxy_routes
        // arbstr extensions (no auth required)
        .route("/v1/stats", get(handlers::stats))
        .route("/v1/requests", get(handlers::logs))
        .route("/health", get(handlers::health))
        .route("/providers", get(handlers::list_providers))
        // State and middleware
        .with_state(state);

    // Apply rate limiting if configured (buffer + rate limit for Clone compatibility)
    if let Some(rps) = rate_limit_rps {
        if rps > 0 {
            tracing::info!(rps = rps, "Rate limiting enabled");
            app = app.layer(
                ServiceBuilder::new()
                    .layer(HandleErrorLayer::new(|_: tower::BoxError| async {
                        axum::http::StatusCode::TOO_MANY_REQUESTS
                    }))
                    .layer(tower::buffer::BufferLayer::new(1024))
                    .layer(tower::limit::RateLimitLayer::new(
                        rps,
                        Duration::from_secs(1),
                    )),
            );
        }
    }

    app.layer(TraceLayer::new_for_http().make_span_with(
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
    ))
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

    // Initialize read-only database pool for stats queries
    let read_db = match &db {
        Some(_) => {
            let db_config = config.database();
            match crate::storage::init_read_pool(&db_config.path).await {
                Ok(pool) => {
                    tracing::info!("Read-only database pool initialized");
                    Some(pool)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to initialize read-only pool, stats disabled");
                    None
                }
            }
        }
        None => None,
    };

    // Initialize bounded DB writer if database is available
    let db_writer = db.as_ref().map(|pool| DbWriter::new(pool.clone()));

    // Initialize circuit breaker registry with one breaker per provider
    let provider_names: Vec<String> = config.providers.iter().map(|p| p.name.clone()).collect();
    let circuit_breakers = Arc::new(CircuitBreakerRegistry::new(&provider_names));

    let state = AppState {
        router: Arc::new(provider_router),
        http_client,
        config: Arc::new(config),
        db,
        read_db,
        db_writer,
        circuit_breakers,
    };

    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!(address = %listen_addr, "Starting arbstr proxy server");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Server shutdown complete");
    Ok(())
}

/// Wait for a shutdown signal (SIGINT or SIGTERM on Unix, Ctrl+C on all platforms).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received SIGINT, starting graceful shutdown"),
        _ = terminate => tracing::info!("Received SIGTERM, starting graceful shutdown"),
    }
}
