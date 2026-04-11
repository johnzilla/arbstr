//! Integration tests for vault fault tolerance.
//!
//! Tests pending settlement reconciliation:
//! - Direct DB insertion tests (deterministic, no HTTP round-trip for the main flow)
//! - Full-cycle integration test (request -> settle failure -> pending row -> reconciliation)

mod common;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::routing::post;
use axum::Router;

use arbstr::proxy::vault::{
    count_pending, fetch_pending, insert_pending_settlement, reconcile_once, PendingSettlement,
    VaultClient,
};

/// A recorded call to the mock vault, with operation name, body, and timestamp.
type CallLog = Arc<Mutex<Vec<(String, serde_json::Value, Instant)>>>;

/// Start a mock vault server where settle/release endpoints return the given status code.
/// Reserve always returns 200 with a reservation_id.
async fn start_mock_vault_for_reconciliation(
    settle_status: u16,
    release_status: u16,
) -> (String, CallLog) {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));

    let settle_log = log.clone();
    let release_log = log.clone();

    let app = Router::new()
        .route(
            "/internal/reserve",
            post({
                let log = log.clone();
                move |headers: axum::http::HeaderMap, body: axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        let token = headers
                            .get("X-Internal-Token")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("");
                        if token != "test-internal-token" {
                            return (
                                axum::http::StatusCode::FORBIDDEN,
                                axum::Json(serde_json::json!({"error": "invalid internal token"})),
                            );
                        }
                        log.lock().unwrap().push((
                            "reserve".to_string(),
                            body.0.clone(),
                            Instant::now(),
                        ));
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({"reservation_id": "res-test-123"})),
                        )
                    }
                }
            }),
        )
        .route(
            "/internal/settle",
            post({
                let log = settle_log;
                let status = settle_status;
                move |body: axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push((
                            "settle".to_string(),
                            body.0.clone(),
                            Instant::now(),
                        ));
                        let status_code = axum::http::StatusCode::from_u16(status).unwrap();
                        if status >= 200 && status < 300 {
                            (
                                status_code,
                                axum::Json(
                                    serde_json::json!({"settled": true, "refunded_msats": 0}),
                                ),
                            )
                        } else {
                            (
                                status_code,
                                axum::Json(serde_json::json!({"error": "mock settle error"})),
                            )
                        }
                    }
                }
            }),
        )
        .route(
            "/internal/release",
            post({
                let log = release_log;
                let status = release_status;
                move |body: axum::Json<serde_json::Value>| {
                    let log = log.clone();
                    async move {
                        log.lock().unwrap().push((
                            "release".to_string(),
                            body.0.clone(),
                            Instant::now(),
                        ));
                        let status_code = axum::http::StatusCode::from_u16(status).unwrap();
                        if status >= 200 && status < 300 {
                            (
                                status_code,
                                axum::Json(serde_json::json!({"released": true})),
                            )
                        } else {
                            (
                                status_code,
                                axum::Json(serde_json::json!({"error": "mock release error"})),
                            )
                        }
                    }
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (url, log)
}

/// Create a VaultClient pointing at the given mock vault URL.
fn create_test_vault_client(vault_url: &str) -> VaultClient {
    let config = arbstr::config::VaultConfig {
        url: vault_url.to_string(),
        internal_token: "test-internal-token".into(),
        default_reserve_tokens: 4096,
        pending_threshold: 100,
    };
    VaultClient::new(reqwest::Client::new(), &config)
}

// ============================================================================
// Test 1: Successful replay of pending settlements
// ============================================================================

#[tokio::test]
async fn test_reconcile_replays_pending_settlements() {
    let pool = common::setup_test_db().await;
    let (vault_url, vault_log) = start_mock_vault_for_reconciliation(200, 200).await;
    let vault = create_test_vault_client(&vault_url);

    // Insert 3 pending settlements (2 settle, 1 release)
    let settlements = vec![
        PendingSettlement {
            settlement_type: "settle".to_string(),
            reservation_id: "res-001".to_string(),
            amount_msats: Some(5000),
            metadata: serde_json::to_string(&serde_json::json!({
                "tokens_in": 10, "tokens_out": 20, "provider": "alpha", "latency_ms": 100
            }))
            .unwrap(),
        },
        PendingSettlement {
            settlement_type: "settle".to_string(),
            reservation_id: "res-002".to_string(),
            amount_msats: Some(3000),
            metadata: serde_json::to_string(&serde_json::json!({
                "tokens_in": 5, "tokens_out": 10, "provider": "beta", "latency_ms": 50
            }))
            .unwrap(),
        },
        PendingSettlement {
            settlement_type: "release".to_string(),
            reservation_id: "res-003".to_string(),
            amount_msats: None,
            metadata: "provider failure".to_string(),
        },
    ];

    for s in &settlements {
        insert_pending_settlement(&pool, s).await.unwrap();
    }

    assert_eq!(count_pending(&pool).await.unwrap(), 3);

    // Run reconciliation -- vault returns 200 for all
    let (replayed, failed, evicted) = reconcile_once(&vault, &pool).await;

    assert_eq!(replayed, 3, "all 3 should be replayed");
    assert_eq!(failed, 0, "none should fail");
    assert_eq!(evicted, 0, "none should be evicted");
    assert_eq!(
        count_pending(&pool).await.unwrap(),
        0,
        "all should be deleted"
    );

    // Verify vault received the calls
    let calls = vault_log.lock().unwrap();
    let settle_count = calls.iter().filter(|(op, _, _)| op == "settle").count();
    let release_count = calls.iter().filter(|(op, _, _)| op == "release").count();
    assert_eq!(settle_count, 2, "2 settle calls expected");
    assert_eq!(release_count, 1, "1 release call expected");
}

// ============================================================================
// Test 2: Failed replay increments attempts
// ============================================================================

#[tokio::test]
async fn test_reconcile_increments_attempts_on_failure() {
    let pool = common::setup_test_db().await;
    // Vault returns 500 for settle
    let (vault_url, _vault_log) = start_mock_vault_for_reconciliation(500, 500).await;
    let vault = create_test_vault_client(&vault_url);

    let settlement = PendingSettlement {
        settlement_type: "settle".to_string(),
        reservation_id: "res-fail-001".to_string(),
        amount_msats: Some(5000),
        metadata: serde_json::to_string(&serde_json::json!({
            "tokens_in": 10, "tokens_out": 20, "provider": "alpha", "latency_ms": 100
        }))
        .unwrap(),
    };

    insert_pending_settlement(&pool, &settlement).await.unwrap();

    let (replayed, failed, evicted) = reconcile_once(&vault, &pool).await;

    assert_eq!(replayed, 0, "none should be replayed");
    assert_eq!(failed, 1, "1 should fail");
    assert_eq!(evicted, 0, "none should be evicted");

    // Row should still exist with attempts incremented
    assert_eq!(
        count_pending(&pool).await.unwrap(),
        1,
        "row should still exist"
    );

    let pending = fetch_pending(&pool).await.unwrap();
    assert_eq!(pending.len(), 1);
    let (_id, _settlement, attempts) = &pending[0];
    assert_eq!(*attempts, 1, "attempts should be incremented to 1");
}

// ============================================================================
// Test 3: Eviction after max attempts
// ============================================================================

#[tokio::test]
async fn test_reconcile_evicts_after_max_attempts() {
    let pool = common::setup_test_db().await;
    // Vault returns 200 but should never be called for evicted settlements
    let (vault_url, vault_log) = start_mock_vault_for_reconciliation(200, 200).await;
    let vault = create_test_vault_client(&vault_url);

    let settlement = PendingSettlement {
        settlement_type: "settle".to_string(),
        reservation_id: "res-stale-001".to_string(),
        amount_msats: Some(5000),
        metadata: serde_json::to_string(&serde_json::json!({
            "tokens_in": 10, "tokens_out": 20, "provider": "alpha", "latency_ms": 100
        }))
        .unwrap(),
    };

    insert_pending_settlement(&pool, &settlement).await.unwrap();

    // Manually set attempts to 10 (max)
    sqlx::query("UPDATE pending_settlements SET attempts = 10 WHERE reservation_id = ?")
        .bind("res-stale-001")
        .execute(&pool)
        .await
        .unwrap();

    let (replayed, failed, evicted) = reconcile_once(&vault, &pool).await;

    assert_eq!(replayed, 0, "none should be replayed");
    assert_eq!(failed, 0, "none should fail");
    assert_eq!(evicted, 1, "1 should be evicted");
    assert_eq!(
        count_pending(&pool).await.unwrap(),
        0,
        "evicted row should be deleted"
    );

    // Verify NO HTTP calls were made to vault (eviction skips replay)
    let calls = vault_log.lock().unwrap();
    let settle_count = calls.iter().filter(|(op, _, _)| op == "settle").count();
    let release_count = calls.iter().filter(|(op, _, _)| op == "release").count();
    assert_eq!(settle_count, 0, "no settle calls for evicted settlement");
    assert_eq!(release_count, 0, "no release calls for evicted settlement");
}

// ============================================================================
// Test 4: Mixed eviction and replay
// ============================================================================

#[tokio::test]
async fn test_reconcile_mixed_eviction_and_replay() {
    let pool = common::setup_test_db().await;
    let (vault_url, vault_log) = start_mock_vault_for_reconciliation(200, 200).await;
    let vault = create_test_vault_client(&vault_url);

    // Settlement 1: attempts=9 (should replay normally)
    let s1 = PendingSettlement {
        settlement_type: "settle".to_string(),
        reservation_id: "res-replay-001".to_string(),
        amount_msats: Some(5000),
        metadata: serde_json::to_string(&serde_json::json!({
            "tokens_in": 10, "tokens_out": 20, "provider": "alpha", "latency_ms": 100
        }))
        .unwrap(),
    };
    insert_pending_settlement(&pool, &s1).await.unwrap();
    sqlx::query("UPDATE pending_settlements SET attempts = 9 WHERE reservation_id = ?")
        .bind("res-replay-001")
        .execute(&pool)
        .await
        .unwrap();

    // Settlement 2: attempts=10 (should be evicted)
    let s2 = PendingSettlement {
        settlement_type: "release".to_string(),
        reservation_id: "res-evict-001".to_string(),
        amount_msats: None,
        metadata: "provider failure".to_string(),
    };
    insert_pending_settlement(&pool, &s2).await.unwrap();
    sqlx::query("UPDATE pending_settlements SET attempts = 10 WHERE reservation_id = ?")
        .bind("res-evict-001")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(count_pending(&pool).await.unwrap(), 2);

    let (replayed, failed, evicted) = reconcile_once(&vault, &pool).await;

    assert_eq!(replayed, 1, "1 should be replayed (attempts=9)");
    assert_eq!(failed, 0, "none should fail");
    assert_eq!(evicted, 1, "1 should be evicted (attempts=10)");
    assert_eq!(
        count_pending(&pool).await.unwrap(),
        0,
        "both should be removed"
    );

    // Verify only one settle call (for the replayed one), no release call
    let calls = vault_log.lock().unwrap();
    let settle_count = calls.iter().filter(|(op, _, _)| op == "settle").count();
    let release_count = calls.iter().filter(|(op, _, _)| op == "release").count();
    assert_eq!(settle_count, 1, "1 settle call for replayed settlement");
    assert_eq!(
        release_count, 0,
        "no release call (evicted one was release type)"
    );
}

// ============================================================================
// Test 5: Full cycle - settle failure creates pending row, reconciliation replays
// ============================================================================

#[tokio::test]
async fn test_full_cycle_settle_failure_and_reconciliation() {
    // Phase 1: Start mock vault that returns 200 for reserve but 504 for settle
    let (vault_url, vault_log) = start_mock_vault_for_reconciliation(504, 200).await;
    let (provider_url, _provider_log) = start_mock_provider().await;

    // Build an app with vault + DB so pending settlements get persisted
    let pool = common::setup_test_db().await;
    let app = setup_vault_test_app_with_db(&vault_url, &provider_url, pool.clone());

    // Send a chat completion request
    let request = http::Request::post("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("authorization", "Bearer agent-token-abc")
        .body(axum::body::Body::from(
            serde_json::to_string(&serde_json::json!({
                "model": "gpt-4o",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .unwrap(),
        ))
        .unwrap();

    let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
    let status = response.status();
    assert_eq!(
        status,
        http::StatusCode::OK,
        "response should succeed (settle failure is async)"
    );

    // Wait for the async settle task to fail and insert a pending row
    tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

    let pending_count = count_pending(&pool).await.unwrap();
    assert!(
        pending_count >= 1,
        "at least 1 pending settlement should exist after settle failure, got {}",
        pending_count
    );

    // Phase 2: Start a NEW mock vault that returns 200 for settle
    let (vault_url2, vault_log2) = start_mock_vault_for_reconciliation(200, 200).await;
    let vault2 = create_test_vault_client(&vault_url2);

    // Run reconciliation against the new vault
    let (replayed, failed, evicted) = reconcile_once(&vault2, &pool).await;

    assert!(replayed >= 1, "should replay at least 1 pending settlement");
    assert_eq!(failed, 0, "none should fail with new vault");
    assert_eq!(evicted, 0, "none should be evicted (fresh attempts)");
    assert_eq!(
        count_pending(&pool).await.unwrap(),
        0,
        "all pending settlements should be cleared after reconciliation"
    );

    // Verify the new vault received a settle call
    let calls2 = vault_log2.lock().unwrap();
    let settle_count = calls2.iter().filter(|(op, _, _)| op == "settle").count();
    assert!(
        settle_count >= 1,
        "new vault should have received at least 1 settle call"
    );

    // Also verify the original vault received the failed settle attempt
    let calls1 = vault_log.lock().unwrap();
    let original_settle_count = calls1.iter().filter(|(op, _, _)| op == "settle").count();
    assert!(
        original_settle_count >= 1,
        "original vault should have received at least 1 settle attempt"
    );
}

// ── Helpers for full-cycle test ──

/// Start a mock provider that returns valid completions.
async fn start_mock_provider() -> (String, CallLog) {
    let log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let provider_log = log.clone();

    let app = Router::new().route(
        "/v1/chat/completions",
        post({
            let log = provider_log;
            move |body: axum::Json<serde_json::Value>| {
                let log = log.clone();
                async move {
                    log.lock().unwrap().push((
                        "chat_completion".to_string(),
                        body.0.clone(),
                        Instant::now(),
                    ));
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "id": "chatcmpl-test",
                            "object": "chat.completion",
                            "created": 1234567890,
                            "model": "gpt-4o",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "test response"},
                                "finish_reason": "stop"
                            }],
                            "usage": {
                                "prompt_tokens": 10,
                                "completion_tokens": 20,
                                "total_tokens": 30
                            }
                        })),
                    )
                }
            }
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    (url, log)
}

/// Build a test app with vault billing AND a real DB pool for pending settlement persistence.
fn setup_vault_test_app_with_db(
    vault_url: &str,
    provider_url: &str,
    pool: sqlx::SqlitePool,
) -> axum::Router {
    use arbstr::config::*;
    use arbstr::proxy::{create_router, AppState, CircuitBreakerRegistry};
    use arbstr::router::Router as ProviderRouter;

    let config = Config {
        server: ServerConfig {
            listen: "127.0.0.1:0".to_string(),
            rate_limit_rps: None,
            auth_token: None,
        },
        database: None,
        vault: Some(VaultConfig {
            url: vault_url.to_string(),
            internal_token: "test-internal-token".into(),
            default_reserve_tokens: 4096,
            pending_threshold: 100,
        }),
        providers: vec![ProviderConfig {
            name: "test-provider".to_string(),
            url: format!("{}/v1", provider_url),
            api_key: None,
            models: vec!["gpt-4o".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
            tier: Tier::Standard,
            auto_discover: false,
        }],
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
        db: Some(pool.clone()),
        read_db: Some(pool),
        db_writer: None,
        circuit_breakers: registry,
        vault: Some(vault),
    };

    create_router(state)
}
