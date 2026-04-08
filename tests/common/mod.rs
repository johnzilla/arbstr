#![allow(dead_code)]
//! Shared test helpers used across integration test files.

use std::sync::Arc;

use sqlx::SqlitePool;

use arbstr::config::{Config, PoliciesConfig, ProviderConfig, RoutingConfig, ServerConfig, Tier};
use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry};
use arbstr::router::Router as ProviderRouter;

/// Parse an axum response body as JSON.
pub async fn parse_body(
    response: axum::response::Response,
) -> (http::StatusCode, serde_json::Value) {
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("read body");
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    (status, json)
}

/// Standard provider config for tests.
pub fn test_provider(name: &str) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        url: "https://fake.test/v1".to_string(),
        api_key: None,
        models: vec!["gpt-4o".to_string()],
        input_rate: 5,
        output_rate: 15,
        base_fee: 0,
        tier: Tier::default(),
    }
}

/// Build a test app with custom providers and return the router + circuit breaker registry.
pub fn setup_circuit_test_app(
    providers: Vec<ProviderConfig>,
) -> (axum::Router, Arc<CircuitBreakerRegistry>) {
    let provider_names: Vec<String> = providers.iter().map(|p| p.name.clone()).collect();
    let registry = Arc::new(CircuitBreakerRegistry::new(&provider_names));

    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: None,
        },
        database: None,
        vault: None,
        providers: providers.clone(),
        policies: PoliciesConfig::default(),
        logging: Default::default(),
        routing: RoutingConfig::default(),
    };

    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );

    let state = AppState {
        router: Arc::new(provider_router),
        http_client: reqwest::Client::new(),
        config: Arc::new(config),
        db: None,
        read_db: None,
        db_writer: None,
        circuit_breakers: registry.clone(),
        vault: None,
    };

    let app = create_router(state);
    (app, registry)
}

/// Build a test config with two standard providers (alpha, beta) for DB-backed tests.
pub fn db_test_config() -> Config {
    Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: None,
        },
        database: None,
        vault: None,
        providers: vec![
            ProviderConfig {
                name: "alpha".to_string(),
                url: "https://alpha.test/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()],
                input_rate: 10,
                output_rate: 30,
                base_fee: 1,
                tier: Tier::default(),
            },
            ProviderConfig {
                name: "beta".to_string(),
                url: "https://beta.test/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
                input_rate: 5,
                output_rate: 15,
                base_fee: 0,
                tier: Tier::default(),
            },
        ],
        policies: PoliciesConfig::default(),
        logging: Default::default(),
        routing: RoutingConfig::default(),
    }
}

/// Create an in-memory SQLite pool with migrations applied, and return the
/// pool along with an axum Router ready for `oneshot` requests.
pub async fn setup_db_test_app() -> (axum::Router, SqlitePool) {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory SQLite pool");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    let config = db_test_config();
    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );

    let state = AppState {
        router: Arc::new(provider_router),
        http_client: reqwest::Client::new(),
        config: Arc::new(config),
        db: Some(pool.clone()),
        read_db: Some(pool.clone()),
        db_writer: None,
        circuit_breakers: Arc::new(CircuitBreakerRegistry::new(&[])),
        vault: None,
    };

    let app = create_router(state);
    (app, pool)
}
