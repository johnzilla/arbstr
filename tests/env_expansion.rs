//! Integration tests for the full Config::from_file_with_env pipeline.
//!
//! These tests exercise the end-to-end flow: TOML file -> raw parse -> env var
//! expansion -> final Config with KeySource metadata.
//!
//! Each test uses unique file paths and env var names to avoid parallel test interference.

use arbstr::config::{Config, KeySource};
use std::fs;

/// Test that ${VAR} references in api_key are expanded from environment (ENV-01).
#[test]
fn test_env_expansion_resolves_var() {
    let var_name = "TEST_E2E_06_01_KEY";
    let var_value = "cashuResolved";
    let config_path = "/tmp/arbstr_e2e_06_01.toml";

    unsafe { std::env::set_var(var_name, var_value) };

    let toml_content = format!(
        r#"
[server]
listen = "127.0.0.1:19876"

[[providers]]
name = "env-test"
url = "https://example.com/v1"
api_key = "${{{}}}"
models = ["gpt-4o"]
input_rate = 10
output_rate = 30
"#,
        var_name
    );

    fs::write(config_path, toml_content).expect("Failed to write temp config");

    let result = Config::from_file_with_env(config_path);
    assert!(
        result.is_ok(),
        "from_file_with_env should succeed: {:?}",
        result.err()
    );

    let (config, key_sources) = result.unwrap();

    // Verify the provider's api_key was expanded
    let provider = config
        .providers
        .iter()
        .find(|p| p.name == "env-test")
        .expect("Provider 'env-test' should exist");
    assert_eq!(
        provider.api_key.as_ref().unwrap().expose_secret(),
        var_value,
        "api_key should be expanded from env var"
    );

    // Verify key source is EnvExpanded
    let source = key_sources
        .iter()
        .find(|(name, _)| name == "env-test")
        .map(|(_, s)| s)
        .expect("Key source for 'env-test' should exist");
    assert_eq!(*source, KeySource::EnvExpanded);

    // Cleanup
    unsafe { std::env::remove_var(var_name) };
    let _ = fs::remove_file(config_path);
}

/// Test that missing env vars produce clear errors naming variable and provider (ENV-02).
#[test]
fn test_env_expansion_missing_var_errors() {
    let var_name = "TEST_E2E_06_02_MISSING";
    let config_path = "/tmp/arbstr_e2e_06_02.toml";

    // Ensure the var is definitely not set
    unsafe { std::env::remove_var(var_name) };

    let toml_content = format!(
        r#"
[server]
listen = "127.0.0.1:19877"

[[providers]]
name = "missing-test"
url = "https://example.com/v1"
api_key = "${{{}}}"
models = ["gpt-4o"]
input_rate = 10
output_rate = 30
"#,
        var_name
    );

    fs::write(config_path, toml_content).expect("Failed to write temp config");

    let result = Config::from_file_with_env(config_path);
    assert!(
        result.is_err(),
        "from_file_with_env should fail for missing env var"
    );

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains(var_name),
        "Error should name the variable '{}': {}",
        var_name,
        err
    );
    assert!(
        err.contains("missing-test"),
        "Error should name the provider 'missing-test': {}",
        err
    );

    // Cleanup
    let _ = fs::remove_file(config_path);
}

/// Test that convention-based env var discovery works end-to-end (ENV-03).
#[test]
fn test_env_convention_discovers_key() {
    let var_name = "ARBSTR_CONV_PROVIDER_API_KEY";
    let var_value = "cashuConvention";
    let config_path = "/tmp/arbstr_e2e_06_03.toml";

    unsafe { std::env::set_var(var_name, var_value) };

    let toml_content = r#"
[server]
listen = "127.0.0.1:19878"

[[providers]]
name = "conv-provider"
url = "https://example.com/v1"
models = ["gpt-4o"]
input_rate = 10
output_rate = 30
"#;

    fs::write(config_path, toml_content).expect("Failed to write temp config");

    let result = Config::from_file_with_env(config_path);
    assert!(
        result.is_ok(),
        "from_file_with_env should succeed: {:?}",
        result.err()
    );

    let (config, key_sources) = result.unwrap();

    // Verify the provider's api_key was discovered via convention
    let provider = config
        .providers
        .iter()
        .find(|p| p.name == "conv-provider")
        .expect("Provider 'conv-provider' should exist");
    assert_eq!(
        provider.api_key.as_ref().unwrap().expose_secret(),
        var_value,
        "api_key should be discovered from convention env var"
    );

    // Verify key source is Convention with correct var name
    let source = key_sources
        .iter()
        .find(|(name, _)| name == "conv-provider")
        .map(|(_, s)| s)
        .expect("Key source for 'conv-provider' should exist");
    assert_eq!(
        *source,
        KeySource::Convention(var_name.to_string()),
        "Key source should be Convention with var name '{}'",
        var_name
    );

    // Cleanup
    unsafe { std::env::remove_var(var_name) };
    let _ = fs::remove_file(config_path);
}

/// Test that a provider with no api_key and no convention var produces KeySource::None.
#[test]
fn test_env_no_key_produces_none_source() {
    let provider_name = "nokey-provider";
    let convention_var = "ARBSTR_NOKEY_PROVIDER_API_KEY";
    let config_path = "/tmp/arbstr_e2e_06_04.toml";

    // Ensure the convention var is not set
    unsafe { std::env::remove_var(convention_var) };

    let toml_content = r#"
[server]
listen = "127.0.0.1:19879"

[[providers]]
name = "nokey-provider"
url = "https://example.com/v1"
models = ["gpt-4o"]
input_rate = 10
output_rate = 30
"#;

    fs::write(config_path, toml_content).expect("Failed to write temp config");

    let result = Config::from_file_with_env(config_path);
    assert!(
        result.is_ok(),
        "from_file_with_env should succeed: {:?}",
        result.err()
    );

    let (config, key_sources) = result.unwrap();

    // Verify the provider has no api_key
    let provider = config
        .providers
        .iter()
        .find(|p| p.name == provider_name)
        .expect("Provider 'nokey-provider' should exist");
    assert!(
        provider.api_key.is_none(),
        "api_key should be None when no key is available"
    );

    // Verify key source is None
    let source = key_sources
        .iter()
        .find(|(name, _)| name == provider_name)
        .map(|(_, s)| s)
        .expect("Key source for 'nokey-provider' should exist");
    assert_eq!(*source, KeySource::None);

    // Cleanup
    let _ = fs::remove_file(config_path);
}

/// Test that a literal api_key (no ${} references) passes through unchanged.
#[test]
fn test_env_literal_key_passthrough() {
    let config_path = "/tmp/arbstr_e2e_06_05.toml";

    let toml_content = r#"
[server]
listen = "127.0.0.1:19880"

[[providers]]
name = "literal-test"
url = "https://example.com/v1"
api_key = "cashuLiteral"
models = ["gpt-4o"]
input_rate = 10
output_rate = 30
"#;

    fs::write(config_path, toml_content).expect("Failed to write temp config");

    let result = Config::from_file_with_env(config_path);
    assert!(
        result.is_ok(),
        "from_file_with_env should succeed: {:?}",
        result.err()
    );

    let (config, key_sources) = result.unwrap();

    // Verify the provider's api_key is the literal value
    let provider = config
        .providers
        .iter()
        .find(|p| p.name == "literal-test")
        .expect("Provider 'literal-test' should exist");
    assert_eq!(
        provider.api_key.as_ref().unwrap().expose_secret(),
        "cashuLiteral",
        "api_key should be the literal value from config"
    );

    // Verify key source is Literal
    let source = key_sources
        .iter()
        .find(|(name, _)| name == "literal-test")
        .map(|(_, s)| s)
        .expect("Key source for 'literal-test' should exist");
    assert_eq!(*source, KeySource::Literal);

    // Cleanup
    let _ = fs::remove_file(config_path);
}
