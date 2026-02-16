//! Integration tests for the GET /v1/stats endpoint.
//!
//! Spins up a real axum router with an in-memory SQLite database,
//! seeds it with known request records, and makes HTTP requests via
//! `tower::ServiceExt::oneshot` (no TCP listener needed).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use http::Request;
use sqlx::SqlitePool;
use tower::ServiceExt;

use chrono::{DateTime, SecondsFormat, Utc};

use arbstr::config::{Config, PoliciesConfig, ProviderConfig, ServerConfig};
use arbstr::proxy::{create_router, AppState};
use arbstr::router::Router as ProviderRouter;

/// Format a DateTime<Utc> as RFC 3339 with `Z` suffix (URL-safe, no `+` sign).
fn rfc3339z(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Global counter for generating unique correlation IDs.
static CORRELATION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Build a minimal test config with known providers.
fn test_config() -> Config {
    Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
        },
        database: None,
        providers: vec![
            ProviderConfig {
                name: "alpha".to_string(),
                url: "https://alpha.test/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()],
                input_rate: 10,
                output_rate: 30,
                base_fee: 1,
            },
            ProviderConfig {
                name: "beta".to_string(),
                url: "https://beta.test/v1".to_string(),
                api_key: None,
                models: vec!["gpt-4o-mini".to_string()],
                input_rate: 5,
                output_rate: 15,
                base_fee: 0,
            },
        ],
        policies: PoliciesConfig::default(),
        logging: Default::default(),
    }
}

/// Create an in-memory SQLite pool, run migrations, and return the pool
/// along with an axum Router ready for `oneshot` requests.
async fn setup_test_app() -> (axum::Router, SqlitePool) {
    let pool = SqlitePool::connect("sqlite::memory:")
        .await
        .expect("Failed to create in-memory SQLite pool");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    let config = test_config();
    let provider_router = ProviderRouter::new(
        config.providers.clone(),
        config.policies.rules.clone(),
        config.policies.default_strategy.clone(),
    );

    let http_client = reqwest::Client::new();

    let state = AppState {
        router: Arc::new(provider_router),
        http_client,
        config: Arc::new(config),
        db: Some(pool.clone()),
        read_db: Some(pool.clone()),
    };

    let app = create_router(state);
    (app, pool)
}

/// Insert a request row into the database.
async fn seed_request(
    pool: &SqlitePool,
    timestamp: &str,
    model: &str,
    provider: &str,
    success: bool,
    streaming: bool,
    cost_sats: Option<f64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    latency_ms: i64,
) {
    let correlation_id = format!(
        "test-corr-{}",
        CORRELATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    );

    sqlx::query(
        "INSERT INTO requests (correlation_id, timestamp, model, provider, policy, streaming, \
         input_tokens, output_tokens, cost_sats, provider_cost_sats, latency_ms, success, \
         error_status, error_message) \
         VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, NULL, ?, ?, NULL, NULL)",
    )
    .bind(&correlation_id)
    .bind(timestamp)
    .bind(model)
    .bind(provider)
    .bind(streaming)
    .bind(input_tokens)
    .bind(output_tokens)
    .bind(cost_sats)
    .bind(latency_ms)
    .bind(success)
    .execute(pool)
    .await
    .expect("Failed to seed request");
}

/// Seed the standard test data set:
/// - 3 recent requests (within last 24h)
/// - 1 old request (8 days ago)
async fn seed_standard_data(pool: &SqlitePool) {
    let now = chrono::Utc::now();
    let recent = (now - chrono::Duration::hours(1)).to_rfc3339();
    let old = (now - chrono::Duration::days(8)).to_rfc3339();

    // Recent request 1: gpt-4o / alpha / success / non-streaming
    seed_request(
        pool,
        &recent,
        "gpt-4o",
        "alpha",
        true,
        false,
        Some(10.0),
        Some(100),
        Some(200),
        150,
    )
    .await;

    // Recent request 2: claude-3.5-sonnet / alpha / success / streaming
    seed_request(
        pool,
        &recent,
        "claude-3.5-sonnet",
        "alpha",
        true,
        true,
        Some(20.0),
        Some(150),
        Some(300),
        200,
    )
    .await;

    // Recent request 3: gpt-4o / beta / fail / non-streaming
    seed_request(
        pool, &recent, "gpt-4o", "beta", false, false, None, None, None, 500,
    )
    .await;

    // Old request: gpt-4o / alpha / success / non-streaming (8 days ago)
    seed_request(
        pool,
        &old,
        "gpt-4o",
        "alpha",
        true,
        false,
        Some(5.0),
        Some(50),
        Some(100),
        100,
    )
    .await;
}

/// Helper: parse response body as serde_json::Value.
async fn parse_response(
    response: axum::response::Response,
) -> (http::StatusCode, serde_json::Value) {
    let status = response.status();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("Failed to read response body");
    let value: serde_json::Value =
        serde_json::from_slice(&body_bytes).expect("Failed to parse response JSON");
    (status, value)
}

/// Helper: make a GET request to the given URI on a fresh clone of the app.
async fn get(app: axum::Router, uri: &str) -> (http::StatusCode, serde_json::Value) {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    parse_response(response).await
}

// ──────────────────────────────────────────────────
// Test 1: Default aggregate (no params, default last_7d)
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_aggregate_default() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats").await;

    assert_eq!(status, 200);
    // Default range = last_7d: includes 3 recent, excludes 8-day-old
    assert_eq!(body["counts"]["total"], 3);
    assert_eq!(body["counts"]["success"], 2);
    assert_eq!(body["counts"]["error"], 1);
    assert_eq!(body["counts"]["streaming"], 1);
    // Costs: 10 + 20 = 30 (failed has null cost)
    assert_eq!(body["costs"]["total_cost_sats"], 30.0);
    assert_eq!(body["costs"]["total_input_tokens"], 250);
    assert_eq!(body["costs"]["total_output_tokens"], 500);
    // Performance: avg of (150, 200, 500) = 283.33...
    let avg_latency = body["performance"]["avg_latency_ms"].as_f64().unwrap();
    assert!(
        (avg_latency - 283.333).abs() < 1.0,
        "Expected avg_latency ~283.33, got {}",
        avg_latency
    );
    // Time range fields present
    assert!(body["since"].is_string());
    assert!(body["until"].is_string());
}

// ──────────────────────────────────────────────────
// Test 2: Range preset last_24h
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_aggregate_with_range_last_24h() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?range=last_24h").await;

    assert_eq!(status, 200);
    assert_eq!(body["counts"]["total"], 3);
}

// ──────────────────────────────────────────────────
// Test 3: Range preset last_30d (includes old request)
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_aggregate_with_range_last_30d() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?range=last_30d").await;

    assert_eq!(status, 200);
    assert_eq!(body["counts"]["total"], 4);
    assert_eq!(body["costs"]["total_cost_sats"], 35.0);
}

// ──────────────────────────────────────────────────
// Test 4: Explicit time range (only old request)
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_explicit_time_range() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let now = Utc::now();
    let since = rfc3339z(&(now - chrono::Duration::days(10)));
    let until = rfc3339z(&(now - chrono::Duration::days(2)));

    let uri = format!("/v1/stats?since={}&until={}", since, until);
    let (status, body) = get(app, &uri).await;

    assert_eq!(status, 200);
    assert_eq!(body["counts"]["total"], 1);
    assert_eq!(body["costs"]["total_cost_sats"], 5.0);
}

// ──────────────────────────────────────────────────
// Test 5: Explicit since/until overrides preset
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_explicit_overrides_preset() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let now = Utc::now();
    let since = rfc3339z(&(now - chrono::Duration::days(30)));
    let until = rfc3339z(&(now + chrono::Duration::hours(1)));

    // range=last_1h but since/until encompass all 4 requests -> explicit wins
    let uri = format!("/v1/stats?range=last_1h&since={}&until={}", since, until);
    let (status, body) = get(app, &uri).await;

    assert_eq!(status, 200);
    assert_eq!(body["counts"]["total"], 4);
}

// ──────────────────────────────────────────────────
// Test 6: Filter by model
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_filter_by_model() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?model=gpt-4o&range=last_30d").await;

    assert_eq!(status, 200);
    // gpt-4o: 2 recent + 1 old = 3
    assert_eq!(body["counts"]["total"], 3);
}

// ──────────────────────────────────────────────────
// Test 7: Filter by provider
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_filter_by_provider() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?provider=alpha&range=last_30d").await;

    assert_eq!(status, 200);
    // alpha: 2 recent + 1 old = 3
    assert_eq!(body["counts"]["total"], 3);
}

// ──────────────────────────────────────────────────
// Test 8: Case-insensitive model filter
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_filter_case_insensitive() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?model=GPT-4O&range=last_30d").await;

    assert_eq!(status, 200);
    assert_eq!(body["counts"]["total"], 3);
}

// ──────────────────────────────────────────────────
// Test 9: Non-existent model returns 404
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_filter_nonexistent_model_404() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?model=nonexistent").await;

    assert_eq!(status, 404);
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        message.contains("not found"),
        "Expected 'not found' in error message, got: {}",
        message
    );
}

// ──────────────────────────────────────────────────
// Test 10: Non-existent provider returns 404
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_filter_nonexistent_provider_404() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, _body) = get(app, "/v1/stats?provider=nonexistent").await;

    assert_eq!(status, 404);
}

// ──────────────────────────────────────────────────
// Test 11: group_by=model (per-model breakdown)
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_group_by_model() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(app, "/v1/stats?group_by=model&range=last_30d").await;

    assert_eq!(status, 200);

    // Top-level aggregate still present
    assert_eq!(body["counts"]["total"], 4);

    // Per-model breakdown
    let models = &body["models"];
    assert!(models.is_object(), "Expected 'models' object in response");

    // gpt-4o: 3 requests (2 recent + 1 old)
    assert_eq!(models["gpt-4o"]["counts"]["total"], 3);

    // claude-3.5-sonnet: 1 request
    assert_eq!(models["claude-3.5-sonnet"]["counts"]["total"], 1);

    // gpt-4o-mini: configured in beta but zero traffic
    assert_eq!(models["gpt-4o-mini"]["counts"]["total"], 0);
}

// ──────────────────────────────────────────────────
// Test 12: Empty time range
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_empty_time_range() {
    let (app, pool) = setup_test_app().await;
    seed_standard_data(&pool).await;

    let (status, body) = get(
        app,
        "/v1/stats?since=2020-01-01T00:00:00Z&until=2020-01-02T00:00:00Z",
    )
    .await;

    assert_eq!(status, 200);
    assert_eq!(body["counts"]["total"], 0);
    assert_eq!(body["costs"]["total_cost_sats"], 0.0);
    assert_eq!(body["costs"]["total_input_tokens"], 0);
    assert_eq!(body["costs"]["total_output_tokens"], 0);
    assert_eq!(body["empty"], true);
    assert!(
        body["message"].is_string(),
        "Expected 'message' string when empty"
    );
    let message = body["message"].as_str().unwrap();
    assert!(!message.is_empty(), "Empty message should be non-empty");
}

// ──────────────────────────────────────────────────
// Test 13: Invalid timestamp returns 400
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_invalid_timestamp_400() {
    let (app, _pool) = setup_test_app().await;

    let (status, _body) = get(app, "/v1/stats?since=not-a-date").await;

    assert_eq!(status, 400);
}

// ──────────────────────────────────────────────────
// Test 14: Invalid range preset returns 400
// ──────────────────────────────────────────────────
#[tokio::test]
async fn test_stats_invalid_range_preset_400() {
    let (app, _pool) = setup_test_app().await;

    let (status, _body) = get(app, "/v1/stats?range=last_999d").await;

    assert_eq!(status, 400);
}
