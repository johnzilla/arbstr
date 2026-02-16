//! Integration tests for the GET /v1/requests endpoint.
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

use arbstr::config::{Config, PoliciesConfig, ProviderConfig, ServerConfig};
use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry};
use arbstr::router::Router as ProviderRouter;

/// Global counter for generating unique correlation IDs.
static CORRELATION_COUNTER: AtomicU64 = AtomicU64::new(10_000);

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
                models: vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
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
        circuit_breakers: Arc::new(CircuitBreakerRegistry::new(&[])),
    };

    let app = create_router(state);
    (app, pool)
}

/// Insert a request row into the database.
///
/// Extended from the stats test version to support `stream_duration_ms`,
/// `error_status`, and `error_message` for testing error and streaming records.
#[allow(clippy::too_many_arguments)]
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
    stream_duration_ms: Option<i64>,
    error_status: Option<i32>,
    error_message: Option<&str>,
) {
    let correlation_id = format!(
        "test-logs-corr-{}",
        CORRELATION_COUNTER.fetch_add(1, Ordering::Relaxed)
    );

    sqlx::query(
        "INSERT INTO requests (correlation_id, timestamp, model, provider, policy, streaming, \
         input_tokens, output_tokens, cost_sats, provider_cost_sats, latency_ms, \
         stream_duration_ms, success, error_status, error_message) \
         VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, NULL, ?, ?, ?, ?, ?)",
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
    .bind(stream_duration_ms)
    .bind(success)
    .bind(error_status)
    .bind(error_message)
    .execute(pool)
    .await
    .expect("Failed to seed request");
}

/// Seed the standard logs test data set.
///
/// 5 recent records (within last 24h) + 1 old record (8 days ago):
///   1. gpt-4o / alpha / success / non-streaming / cost=10.0 / latency=100
///   2. gpt-4o / alpha / success / streaming / cost=20.0 / latency=200 / stream_duration=500
///   3. claude-3.5-sonnet / alpha / success / non-streaming / cost=30.0 / latency=300
///   4. gpt-4o / beta / fail / non-streaming / cost=None / latency=500 / error 502
///   5. gpt-4o-mini / beta / success / non-streaming / cost=5.0 / latency=50
///   6. (old) gpt-4o / alpha / success / non-streaming / cost=8.0 / latency=120
async fn seed_logs_data(pool: &SqlitePool) {
    let now = chrono::Utc::now();

    // Use distinct timestamps so sorting by timestamp produces deterministic order.
    // Record 1 is the most recent, record 5 is the oldest of the recent batch.
    let ts1 = (now - chrono::Duration::minutes(10)).to_rfc3339();
    let ts2 = (now - chrono::Duration::minutes(20)).to_rfc3339();
    let ts3 = (now - chrono::Duration::minutes(30)).to_rfc3339();
    let ts4 = (now - chrono::Duration::minutes(40)).to_rfc3339();
    let ts5 = (now - chrono::Duration::minutes(50)).to_rfc3339();
    let ts_old = (now - chrono::Duration::days(8)).to_rfc3339();

    // Record 1: gpt-4o / alpha / success / non-streaming
    seed_request(
        pool,
        &ts1,
        "gpt-4o",
        "alpha",
        true,
        false,
        Some(10.0),
        Some(100),
        Some(200),
        100,
        None,
        None,
        None,
    )
    .await;

    // Record 2: gpt-4o / alpha / success / streaming
    seed_request(
        pool,
        &ts2,
        "gpt-4o",
        "alpha",
        true,
        true,
        Some(20.0),
        Some(150),
        Some(300),
        200,
        Some(500),
        None,
        None,
    )
    .await;

    // Record 3: claude-3.5-sonnet / alpha / success / non-streaming
    seed_request(
        pool,
        &ts3,
        "claude-3.5-sonnet",
        "alpha",
        true,
        false,
        Some(30.0),
        Some(200),
        Some(400),
        300,
        None,
        None,
        None,
    )
    .await;

    // Record 4: gpt-4o / beta / fail / non-streaming / error 502
    seed_request(
        pool,
        &ts4,
        "gpt-4o",
        "beta",
        false,
        false,
        None,
        None,
        None,
        500,
        None,
        Some(502),
        Some("Provider returned 502"),
    )
    .await;

    // Record 5: gpt-4o-mini / beta / success / non-streaming
    seed_request(
        pool,
        &ts5,
        "gpt-4o-mini",
        "beta",
        true,
        false,
        Some(5.0),
        Some(50),
        Some(100),
        50,
        None,
        None,
        None,
    )
    .await;

    // Record 6 (old): gpt-4o / alpha / success / non-streaming (8 days ago)
    seed_request(
        pool,
        &ts_old,
        "gpt-4o",
        "alpha",
        true,
        false,
        Some(8.0),
        Some(80),
        Some(160),
        120,
        None,
        None,
        None,
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
// PAGINATION TESTS (LOG-01)
// ──────────────────────────────────────────────────

/// Test 1: Default pagination (no params, default last_7d)
#[tokio::test]
async fn test_logs_default_page() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests").await;

    assert_eq!(status, 200);
    assert!(body["data"].is_array(), "Expected 'data' to be an array");
    assert_eq!(body["page"], 1);
    assert_eq!(body["per_page"], 20);
    // Default last_7d: includes 5 recent, excludes 8-day-old record
    assert_eq!(body["total"], 5);
    assert_eq!(body["total_pages"], 1);
    assert!(body["since"].is_string(), "Expected 'since' to be a string");
    assert!(body["until"].is_string(), "Expected 'until' to be a string");
}

/// Test 2: Custom page size
#[tokio::test]
async fn test_logs_custom_page_size() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?per_page=2").await;

    assert_eq!(status, 200);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert_eq!(body["per_page"], 2);
    assert_eq!(body["total"], 5);
    assert_eq!(body["total_pages"], 3); // ceil(5/2) = 3
}

/// Test 3: Page 2 of paginated results
#[tokio::test]
async fn test_logs_page_2() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?per_page=2&page=2").await;

    assert_eq!(status, 200);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert_eq!(body["page"], 2);
}

/// Test 4: Out-of-range page returns 200 with empty data
#[tokio::test]
async fn test_logs_out_of_range_page() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?page=999").await;

    assert_eq!(status, 200);
    assert!(body["data"].as_array().unwrap().is_empty());
    assert_eq!(body["page"], 999);
    assert_eq!(body["total"], 5);
}

/// Test 5: per_page clamped to max 100
#[tokio::test]
async fn test_logs_per_page_clamped_to_100() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?per_page=500").await;

    assert_eq!(status, 200);
    assert_eq!(body["per_page"], 100);
}

// ──────────────────────────────────────────────────
// FILTERING TESTS (LOG-02)
// ──────────────────────────────────────────────────

/// Test 6: Filter by model (includes old record with last_30d)
#[tokio::test]
async fn test_logs_filter_by_model() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?model=gpt-4o&range=last_30d").await;

    assert_eq!(status, 200);
    // gpt-4o: records 1, 2, 4 (recent) + 6 (old) = 4
    assert_eq!(body["total"], 4);
    let data = body["data"].as_array().unwrap();
    for entry in data {
        assert_eq!(entry["model"], "gpt-4o");
    }
}

/// Test 7: Filter by provider
#[tokio::test]
async fn test_logs_filter_by_provider() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?provider=beta").await;

    assert_eq!(status, 200);
    // beta: records 4, 5 = 2
    assert_eq!(body["total"], 2);
    let data = body["data"].as_array().unwrap();
    for entry in data {
        assert_eq!(entry["provider"], "beta");
    }
}

/// Test 8: Filter by success=false
#[tokio::test]
async fn test_logs_filter_by_success() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?success=false").await;

    assert_eq!(status, 200);
    // Only record 4 is a failure
    assert_eq!(body["total"], 1);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data[0]["success"], false);
}

/// Test 9: Filter by streaming=true
#[tokio::test]
async fn test_logs_filter_by_streaming() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?streaming=true").await;

    assert_eq!(status, 200);
    // Only record 2 is streaming
    assert_eq!(body["total"], 1);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data[0]["streaming"], true);
}

/// Test 10: Combined filters (model + provider)
#[tokio::test]
async fn test_logs_combined_filters() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?model=gpt-4o&provider=alpha").await;

    assert_eq!(status, 200);
    // gpt-4o from alpha (recent): records 1, 2 = 2
    assert_eq!(body["total"], 2);
    let data = body["data"].as_array().unwrap();
    for entry in data {
        assert_eq!(entry["model"], "gpt-4o");
        assert_eq!(entry["provider"], "alpha");
    }
}

/// Test 11: Non-existent model returns 404
#[tokio::test]
async fn test_logs_filter_nonexistent_model_404() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, _body) = get(app, "/v1/requests?model=nonexistent").await;

    assert_eq!(status, 404);
}

/// Test 12: Non-existent provider returns 404
#[tokio::test]
async fn test_logs_filter_nonexistent_provider_404() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, _body) = get(app, "/v1/requests?provider=nonexistent").await;

    assert_eq!(status, 404);
}

/// Test 13: Time range last_30d includes old record
#[tokio::test]
async fn test_logs_time_range_last_30d() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?range=last_30d").await;

    assert_eq!(status, 200);
    // last_30d includes all 6 records
    assert_eq!(body["total"], 6);
}

// ──────────────────────────────────────────────────
// SORTING TESTS (LOG-03)
// ──────────────────────────────────────────────────

/// Test 14: Sort by cost ascending
#[tokio::test]
async fn test_logs_sort_by_cost_asc() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?sort=cost_sats&order=asc").await;

    assert_eq!(status, 200);
    let data = body["data"].as_array().unwrap();
    assert!(!data.is_empty());

    // Verify ascending order: collect non-null costs and check they are sorted
    let costs: Vec<Option<f64>> = data
        .iter()
        .map(|entry| entry["cost"]["sats"].as_f64())
        .collect();

    // Check that non-null costs are in ascending order
    let non_null: Vec<f64> = costs.iter().filter_map(|c| *c).collect();
    for window in non_null.windows(2) {
        assert!(
            window[0] <= window[1],
            "Expected ascending cost order, got {} before {}",
            window[0],
            window[1]
        );
    }
}

/// Test 15: Sort by latency descending
#[tokio::test]
async fn test_logs_sort_by_latency_desc() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?sort=latency_ms&order=desc").await;

    assert_eq!(status, 200);
    let data = body["data"].as_array().unwrap();
    assert!(!data.is_empty());

    // First record should have the highest latency (500ms = record 4)
    let first_latency = data[0]["timing"]["latency_ms"].as_i64().unwrap();
    assert_eq!(
        first_latency, 500,
        "Expected highest latency (500) first, got {}",
        first_latency
    );

    // Verify descending order
    let latencies: Vec<i64> = data
        .iter()
        .map(|entry| entry["timing"]["latency_ms"].as_i64().unwrap())
        .collect();
    for window in latencies.windows(2) {
        assert!(
            window[0] >= window[1],
            "Expected descending latency order, got {} before {}",
            window[0],
            window[1]
        );
    }
}

/// Test 16: Invalid sort field returns 400
#[tokio::test]
async fn test_logs_invalid_sort_field_400() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?sort=invalid").await;

    assert_eq!(status, 400);
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        message.contains("valid options"),
        "Expected 'valid options' in error message, got: {}",
        message
    );
}

/// Test 17: Invalid sort order returns 400
#[tokio::test]
async fn test_logs_invalid_sort_order_400() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?sort=timestamp&order=sideways").await;

    assert_eq!(status, 400);
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    assert!(
        message.contains("valid options"),
        "Expected 'valid options' in error message, got: {}",
        message
    );
}

// ──────────────────────────────────────────────────
// RESPONSE STRUCTURE TESTS
// ──────────────────────────────────────────────────

/// Test 18: Verify nested response structure on a success record
#[tokio::test]
async fn test_logs_response_structure() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?per_page=1").await;

    assert_eq!(status, 200);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);

    let entry = &data[0];

    // Top-level fields
    assert!(entry["id"].is_i64(), "Expected 'id' to be an integer");
    assert!(
        entry["timestamp"].is_string(),
        "Expected 'timestamp' to be a string"
    );
    assert!(
        entry["model"].is_string(),
        "Expected 'model' to be a string"
    );
    assert!(
        entry["success"].is_boolean(),
        "Expected 'success' to be a boolean"
    );
    assert!(
        entry["streaming"].is_boolean(),
        "Expected 'streaming' to be a boolean"
    );

    // Nested tokens section
    let tokens = &entry["tokens"];
    assert!(
        tokens.is_object(),
        "Expected 'tokens' to be an object, got: {}",
        tokens
    );
    assert!(
        tokens.get("input").is_some(),
        "Expected 'tokens.input' field"
    );
    assert!(
        tokens.get("output").is_some(),
        "Expected 'tokens.output' field"
    );

    // Nested cost section
    let cost = &entry["cost"];
    assert!(
        cost.is_object(),
        "Expected 'cost' to be an object, got: {}",
        cost
    );
    assert!(cost.get("sats").is_some(), "Expected 'cost.sats' field");

    // Nested timing section
    let timing = &entry["timing"];
    assert!(
        timing.is_object(),
        "Expected 'timing' to be an object, got: {}",
        timing
    );
    assert!(
        timing.get("latency_ms").is_some(),
        "Expected 'timing.latency_ms' field"
    );
    // stream_duration_ms may or may not be present (skip_serializing_if)
}

/// Test 19: Error section present on failed request
#[tokio::test]
async fn test_logs_error_section_present_on_failure() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?success=false").await;

    assert_eq!(status, 200);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);

    let entry = &data[0];
    assert_eq!(entry["success"], false);

    // Error section should be present
    let error = &entry["error"];
    assert!(
        error.is_object(),
        "Expected 'error' object on failed request, got: {}",
        error
    );
    assert_eq!(error["status"], 502);
    assert_eq!(error["message"], "Provider returned 502");
}

/// Test 20: Error section absent on successful request
#[tokio::test]
async fn test_logs_error_section_absent_on_success() {
    let (app, pool) = setup_test_app().await;
    seed_logs_data(&pool).await;

    let (status, body) = get(app, "/v1/requests?success=true&per_page=1").await;

    assert_eq!(status, 200);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 1);

    let entry = &data[0];
    assert_eq!(entry["success"], true);

    // Error section should NOT be present (skip_serializing_if = None)
    assert!(
        entry.get("error").is_none() || entry["error"].is_null(),
        "Expected no 'error' key on successful request, got: {}",
        entry
    );
}
