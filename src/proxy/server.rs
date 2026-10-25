//! HTTP server setup and configuration.

use axum::{
    error_handling::HandleErrorLayer,
    extract::State,
    http::StatusCode,
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

use super::discovery;
use uuid::Uuid;

use super::circuit_breaker::CircuitBreakerRegistry;
use super::handlers;
use super::tokenstats::{self, MarketState};
use super::vault::VaultClient;
use crate::config::{ApiKey, Config};
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
    /// Vault treasury client. When Some, requests require vault billing.
    /// When None, arbstr runs in free proxy mode.
    pub vault: Option<VaultClient>,
    /// tokenstats market feed status (skipped sources, last poll, etc.).
    pub market: MarketState,
}

/// Extract the token from an `Authorization: Bearer <token>` header.
fn bearer_token(request: &axum::http::Request<axum::body::Body>) -> Option<&str> {
    request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Compare tokens without short-circuiting on the first differing byte.
fn tokens_match(provided: &str, expected: &str) -> bool {
    let (a, b) = (provided.as_bytes(), expected.as_bytes());
    if b.is_empty() {
        return false;
    }
    // Walk the provided token, cycling through the expected one, and fold the
    // length check into the result instead of returning early: the time taken
    // depends only on the caller's own input, so it reveals neither the
    // expected token's bytes nor its length.
    let diff = a
        .iter()
        .enumerate()
        .fold((a.len() != b.len()) as u8, |acc, (i, x)| {
            acc | (x ^ b[i % b.len()])
        });
    diff == 0
}

fn json_error(status: StatusCode, error: serde_json::Value) -> Response {
    Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&serde_json::json!({ "error": error })).unwrap(),
        ))
        .unwrap()
}

fn invalid_token_response() -> Response {
    json_error(
        StatusCode::UNAUTHORIZED,
        serde_json::json!({
            "message": "Invalid or missing bearer token",
            "type": "authentication_error",
            "code": "invalid_api_key"
        }),
    )
}

/// Middleware that requires `Authorization: Bearer <expected_token>`.
///
/// Used for `server.auth_token` on the proxy endpoints (free proxy mode) and
/// for `server.operator_token` on the analytics endpoints.
async fn auth_middleware(
    expected_token: Arc<ApiKey>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    match bearer_token(&request) {
        Some(token) if tokens_match(token, expected_token.expose_secret()) => {
            next.run(request).await
        }
        _ => invalid_token_response(),
    }
}

/// Middleware for the analytics endpoints in vault mode when no
/// `operator_token` is configured: refuse, and say how to enable them.
async fn operator_token_required(
    _request: axum::http::Request<axum::body::Body>,
    _next: middleware::Next,
) -> Response {
    json_error(
        StatusCode::UNAUTHORIZED,
        serde_json::json!({
            "message": "This endpoint needs [server] operator_token, which is required when [vault] is configured",
            "type": "authentication_error",
            "code": "operator_token_not_configured"
        }),
    )
}

/// Middleware for cost-free proxy endpoints in vault mode: the bearer token
/// must be an agent token vault recognizes. Nothing is reserved.
async fn agent_token_middleware(
    State(state): State<AppState>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    let Some(vault) = state.vault.as_ref() else {
        return next.run(request).await;
    };
    let Some(token) = bearer_token(&request) else {
        return json_error(
            StatusCode::UNAUTHORIZED,
            serde_json::json!({
                "message": "Authorization: Bearer <token> required",
                "type": "authentication_error",
                "code": "invalid_api_key"
            }),
        );
    };
    match vault.verify_agent(token).await {
        Ok(()) => next.run(request).await,
        Err(e) => {
            let status = e.status_code();
            tracing::warn!(error = %e, status, "Vault agent verify failed");
            json_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                serde_json::json!({
                    "message": e.to_string(),
                    "type": "billing_error",
                    "code": format!("vault_{}", status)
                }),
            )
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
    let auth_token = state.config.server.auth_token.clone().map(Arc::new);
    let operator_token = state.config.server.operator_token.clone().map(Arc::new);
    let has_vault = state.vault.is_some();

    let require = |token: Arc<ApiKey>| {
        middleware::from_fn(move |req, next| auth_middleware(token.clone(), req, next))
    };

    // Chat completions: in vault mode the reserve call authenticates the agent.
    let mut chat_routes =
        Router::new().route("/v1/chat/completions", post(handlers::chat_completions));
    // Cost-free proxy endpoints: in vault mode, a vault-verified agent token.
    let mut client_routes = Router::new()
        .route("/v1/models", get(handlers::list_models))
        .route("/v1/cost", post(handlers::cost_estimate));
    // Analytics over every agent's traffic: the operator's token.
    let mut operator_routes = Router::new()
        .route("/v1/stats", get(handlers::stats))
        .route("/v1/requests", get(handlers::logs));

    if has_vault {
        client_routes = client_routes.layer(middleware::from_fn_with_state(
            state.clone(),
            agent_token_middleware,
        ));
        operator_routes = match operator_token {
            Some(token) => operator_routes.layer(require(token)),
            None => {
                tracing::warn!(
                    "[server] operator_token is not set: /v1/stats and /v1/requests will refuse all requests"
                );
                operator_routes.layer(middleware::from_fn(operator_token_required))
            }
        };
    } else {
        if let Some(token) = auth_token.clone() {
            chat_routes = chat_routes.layer(require(token.clone()));
            client_routes = client_routes.layer(require(token));
        }
        // operator_token, else auth_token, else open (local free proxy)
        if let Some(token) = operator_token.or(auth_token) {
            operator_routes = operator_routes.layer(require(token));
        }
    }

    let mut app = chat_routes
        .merge(client_routes)
        .merge(operator_routes)
        // Always open: liveness and the public provider/rate list
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
pub async fn run_server(mut config: Config) -> anyhow::Result<()> {
    let listen_addr = config.server.listen.clone();

    // Create HTTP client with reasonable defaults (needed for discovery before router init)
    let http_client = Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(10))
        .build()?;

    // Discover models for auto_discover providers
    discovery::discover_models(&mut config.providers, &http_client).await;

    // Create provider router (after discovery so models are populated)
    let blend_ratio = config
        .tokenstats
        .as_ref()
        .map(|t| t.blend_ratio)
        .unwrap_or(3.0);
    let provider_router = ProviderRouter::with_blend_ratio(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
        blend_ratio,
    );

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

    // Initialize vault client if configured
    let vault = config.vault.as_ref().map(|vault_config| {
        tracing::info!(url = %vault_config.url, "Vault treasury integration enabled");
        VaultClient::new(http_client.clone(), vault_config)
    });

    let market = MarketState::new();
    let static_providers = config.providers.clone();
    let tokenstats_cfg = config.tokenstats.clone();

    let state = AppState {
        router: Arc::new(provider_router),
        http_client: http_client.clone(),
        config: Arc::new(config),
        db,
        read_db,
        db_writer,
        circuit_breakers,
        vault,
        market: market.clone(),
    };

    // Spawn reconciliation task if vault is configured and DB is available
    let reconciliation_cancel =
        if let (Some(vault_client), Some(db_pool)) = (&state.vault, &state.db) {
            let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
            let vault_clone = vault_client.clone();
            let pool_clone = db_pool.clone();
            tokio::spawn(super::vault::reconciliation_loop(
                vault_clone,
                pool_clone,
                Duration::from_secs(60),
                cancel_rx,
            ));
            tracing::info!("Vault reconciliation task started (60s interval)");
            Some(cancel_tx)
        } else {
            None
        };

    // Spawn tokenstats market poller when configured
    let tokenstats_cancel = if let Some(ts_cfg) = tokenstats_cfg {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let router_clone = (*state.router).clone();
        let client_clone = http_client;
        let market_clone = market;
        tracing::info!(url = %ts_cfg.url, interval = ts_cfg.poll_interval_secs, "tokenstats market feed enabled");
        tokio::spawn(tokenstats::poll_loop(
            client_clone,
            ts_cfg,
            static_providers,
            router_clone,
            market_clone,
            cancel_rx,
        ));
        Some(cancel_tx)
    } else {
        None
    };

    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    tracing::info!(address = %listen_addr, "Starting arbstr proxy server");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    // Signal background tasks to stop
    if let Some(cancel_tx) = tokenstats_cancel {
        let _ = cancel_tx.send(true);
    }
    if let Some(cancel_tx) = reconciliation_cancel {
        let _ = cancel_tx.send(true);
        // Give it a moment to complete the final reconciliation pass
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

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

#[cfg(test)]
mod tests {
    use super::tokens_match;

    #[test]
    fn tokens_match_requires_exact_token() {
        assert!(tokens_match("secret-token", "secret-token"));
        assert!(!tokens_match("secret-tokeN", "secret-token"));
        assert!(!tokens_match("secret-token-x", "secret-token"));
        assert!(!tokens_match("secret", "secret-token"));
        assert!(!tokens_match("", "secret-token"));
        // A repeat of the expected token must not match by wrapping around.
        assert!(!tokens_match("abcabc", "abc"));
        assert!(!tokens_match("", ""));
    }
}
