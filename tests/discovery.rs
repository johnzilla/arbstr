//! Integration tests for model discovery from provider /v1/models endpoints.

use arbstr::config::{Config, ProviderConfig, Tier};
use arbstr::proxy::discovery::discover_models;
use reqwest::Client;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_provider(name: &str, url: &str, auto_discover: bool, models: Vec<String>) -> ProviderConfig {
    ProviderConfig {
        name: name.to_string(),
        url: url.to_string(),
        api_key: None,
        models,
        input_rate: 0,
        output_rate: 0,
        base_fee: 0,
        tier: Tier::Local,
        auto_discover,
    }
}

/// Test 1: Provider with auto_discover=true and a reachable /v1/models endpoint
/// gets its models list replaced with discovered model ids.
#[tokio::test]
async fn discovery_success() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "model-a", "object": "model"},
                {"id": "model-b", "object": "model"}
            ]
        })))
        .mount(&mock_server)
        .await;

    let mut providers = vec![test_provider(
        "test-discoverable",
        &format!("{}/v1", mock_server.uri()),
        true,
        vec!["fallback".to_string()],
    )];

    let client = Client::new();
    discover_models(&mut providers, &client).await;

    assert_eq!(providers[0].models, vec!["model-a", "model-b"]);
}

/// Test 2: Provider with auto_discover=true but unreachable endpoint keeps
/// its static models list unchanged.
#[tokio::test]
async fn discovery_unreachable() {
    let mut providers = vec![test_provider(
        "unreachable",
        "http://127.0.0.1:1", // unreachable port
        true,
        vec!["static-model".to_string()],
    )];

    let client = Client::new();
    discover_models(&mut providers, &client).await;

    assert_eq!(providers[0].models, vec!["static-model"]);
}

/// Test 3: Provider with auto_discover=true, unreachable endpoint, and empty
/// models stays empty (won't match requests). Per D-02.
#[tokio::test]
async fn discovery_unreachable_empty() {
    let mut providers = vec![test_provider(
        "unreachable-empty",
        "http://127.0.0.1:1",
        true,
        vec![],
    )];

    let client = Client::new();
    discover_models(&mut providers, &client).await;

    assert!(providers[0].models.is_empty());
}

/// Test 4: Provider with auto_discover=false (or field absent) is never polled.
/// Its models list is unchanged regardless of endpoint availability.
#[tokio::test]
async fn discovery_skipped() {
    let mock_server = MockServer::start().await;

    // Mount a mock that would change models if called - but it should NOT be called
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "should-not-appear", "object": "model"}
            ]
        })))
        .expect(0) // Assert this mock is never called
        .mount(&mock_server)
        .await;

    let mut providers = vec![test_provider(
        "no-discover",
        &format!("{}/v1", mock_server.uri()),
        false,
        vec!["original-model".to_string()],
    )];

    let client = Client::new();
    discover_models(&mut providers, &client).await;

    assert_eq!(providers[0].models, vec!["original-model"]);
}

/// Test 5: A TOML config without auto_discover field deserializes successfully
/// with auto_discover=false. Per D-05.
#[test]
fn config_backward_compat() {
    let toml_str = r#"
[server]
listen = "127.0.0.1:8080"

[database]
path = "./test.db"

[[providers]]
name = "legacy-provider"
url = "https://example.com/v1"
models = ["gpt-4o"]
input_rate = 10
output_rate = 30

[policies]
default_strategy = "cheapest"
"#;

    let config: Config = toml::from_str(toml_str).expect("should parse without auto_discover");
    assert!(!config.providers[0].auto_discover);
}

/// Test 6: Discovered models fully replace static list (not merge/append). Per D-03.
#[tokio::test]
async fn discovery_replaces_static() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [
                {"id": "new-model", "object": "model"}
            ]
        })))
        .mount(&mock_server)
        .await;

    let mut providers = vec![test_provider(
        "replace-test",
        &format!("{}/v1", mock_server.uri()),
        true,
        vec!["old-model-a".to_string(), "old-model-b".to_string()],
    )];

    let client = Client::new();
    discover_models(&mut providers, &client).await;

    // Should be exactly the discovered models, not a merge
    assert_eq!(providers[0].models, vec!["new-model"]);
    assert!(!providers[0].models.contains(&"old-model-a".to_string()));
    assert!(!providers[0].models.contains(&"old-model-b".to_string()));
}
