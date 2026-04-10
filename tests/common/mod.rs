#![allow(dead_code)]
//! Shared test helpers used across integration test files.

use std::sync::Arc;

use sqlx::SqlitePool;

use arbstr::config::{
    Config, PoliciesConfig, ProviderConfig, RoutingConfig, ServerConfig, Tier, VaultConfig,
};
use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry};
use arbstr::proxy::vault::VaultClient;
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
        auto_discover: false,
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
                auto_discover: false,
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
                auto_discover: false,
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

/// Build a test app with vault billing enabled, connecting to a mock vault at the given URL
/// and mock provider at the given URL. Returns the router.
pub fn setup_vault_test_app(vault_url: &str, provider_url: &str) -> axum::Router {
    setup_vault_test_app_with_auth(vault_url, provider_url, None)
}

/// Build a test app with vault billing enabled AND an optional server auth_token.
/// When vault is configured, server auth middleware is bypassed (D-01).
pub fn setup_vault_test_app_with_auth(
    vault_url: &str,
    provider_url: &str,
    auth_token: Option<&str>,
) -> axum::Router {
    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: auth_token.map(|s| s.to_string()),
        },
        database: None,
        vault: Some(VaultConfig {
            url: vault_url.to_string(),
            internal_token: "test-internal-token".into(),
            default_reserve_tokens: 4096,
            pending_threshold: 100,
        }),
        providers: vec![
            ProviderConfig {
                name: "cheap-local".to_string(),
                url: format!("{}/v1", provider_url),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 1,
                output_rate: 5,
                base_fee: 0,
                tier: Tier::Local,
                auto_discover: false,
            },
            ProviderConfig {
                name: "expensive-frontier".to_string(),
                url: format!("{}/v1", provider_url),
                api_key: None,
                models: vec!["gpt-4o".to_string()],
                input_rate: 10,
                output_rate: 30,
                base_fee: 2,
                tier: Tier::Frontier,
                auto_discover: false,
            },
        ],
        policies: PoliciesConfig::default(),
        logging: Default::default(),
        routing: RoutingConfig::default(),
    };

    let provider_names: Vec<String> = config.providers.iter().map(|p| p.name.clone()).collect();
    let registry = Arc::new(CircuitBreakerRegistry::new(&provider_names));

    let vault_config = config.vault.as_ref().unwrap();
    let vault = VaultClient::new(reqwest::Client::new(), vault_config);

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
        circuit_breakers: registry,
        vault: Some(vault),
    };

    create_router(state)
}

/// Create an in-memory SQLite pool with migrations applied for direct DB tests.
pub async fn setup_test_db() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory SQLite pool");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    pool
}

/// Build a test app WITHOUT vault (free proxy mode) but with a real mock provider URL.
pub fn setup_free_proxy_test_app(provider_url: &str) -> axum::Router {
    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: None,
        },
        database: None,
        vault: None,
        providers: vec![ProviderConfig {
            name: "local-provider".to_string(),
            url: format!("{}/v1", provider_url),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 1,
            output_rate: 5,
            base_fee: 0,
            tier: Tier::Local,
            auto_discover: false,
        }],
        policies: PoliciesConfig::default(),
        logging: Default::default(),
        routing: RoutingConfig::default(),
    };

    let provider_names: Vec<String> = config.providers.iter().map(|p| p.name.clone()).collect();
    let registry = Arc::new(CircuitBreakerRegistry::new(&provider_names));

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
        circuit_breakers: registry,
        vault: None,
    };

    create_router(state)
}
