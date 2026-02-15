//! Configuration parsing and validation for arbstr.

use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::Path;

/// Root configuration structure.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: Option<DatabaseConfig>,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub policies: PoliciesConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

/// HTTP server configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Address to listen on (e.g., "127.0.0.1:8080")
    #[serde(default = "default_listen")]
    pub listen: String,
}

fn default_listen() -> String {
    "127.0.0.1:8080".to_string()
}

/// Database configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    /// Path to SQLite database file
    #[serde(default = "default_db_path")]
    pub path: String,
}

fn default_db_path() -> String {
    "./arbstr.db".to_string()
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: default_db_path(),
        }
    }
}

/// API key wrapper that redacts in Debug/Display/Serialize and zeroizes on drop.
///
/// The inner `SecretString` ensures the key value is:
/// - Zeroized in memory when dropped (SEC-02)
/// - Never exposed via Debug or Display (SEC-01)
/// - Only accessible via `.expose_secret()` (grep-auditable)
#[derive(Clone)]
pub struct ApiKey(SecretString);

impl ApiKey {
    /// Access the raw key value. Every call site is auditable via `grep expose_secret`.
    pub fn expose_secret(&self) -> &str {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl std::fmt::Display for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED]")
    }
}

impl Serialize for ApiKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("[REDACTED]")
    }
}

impl<'de> serde::Deserialize<'de> for ApiKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|s| ApiKey(SecretString::from(s)))
    }
}

impl From<String> for ApiKey {
    fn from(s: String) -> Self {
        ApiKey(SecretString::from(s))
    }
}

impl From<&str> for ApiKey {
    fn from(s: &str) -> Self {
        ApiKey(SecretString::from(s))
    }
}

/// How a provider's API key was resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum KeySource {
    /// Key was a literal string in config (no ${} references)
    Literal,
    /// Key contained ${VAR} references expanded from environment
    EnvExpanded,
    /// Key was auto-discovered from convention env var (holds var name)
    Convention(String),
    /// No key available
    None,
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Literal => write!(f, "config-literal"),
            KeySource::EnvExpanded => write!(f, "env-expanded"),
            KeySource::Convention(var) => write!(f, "convention ({})", var),
            KeySource::None => write!(f, "none"),
        }
    }
}

/// Provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    /// Unique name for this provider
    pub name: String,
    /// Base URL for the provider's API (e.g., "https://api.routstr.com/v1")
    pub url: String,
    /// Optional API key or Cashu token
    pub api_key: Option<ApiKey>,
    /// Models supported by this provider
    #[serde(default)]
    pub models: Vec<String>,
    /// Input token rate in sats per 1000 tokens
    #[serde(default)]
    pub input_rate: u64,
    /// Output token rate in sats per 1000 tokens
    #[serde(default)]
    pub output_rate: u64,
    /// Base fee per request in sats
    #[serde(default)]
    pub base_fee: u64,
}

/// Policies configuration.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PoliciesConfig {
    /// Default routing strategy
    #[serde(default = "default_strategy")]
    pub default_strategy: String,
    /// Policy rules
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

fn default_strategy() -> String {
    "cheapest".to_string()
}

/// A single policy rule.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicyRule {
    /// Policy name (matched via X-Arbstr-Policy header)
    pub name: String,
    /// Allowed models for this policy
    #[serde(default)]
    pub allowed_models: Vec<String>,
    /// Routing strategy: "lowest_cost", "lowest_latency", "round_robin"
    #[serde(default = "default_strategy")]
    pub strategy: String,
    /// Maximum cost in sats per 1000 output tokens
    pub max_sats_per_1k_output: Option<u64>,
    /// Keywords for heuristic matching
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// Logging configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    /// Log level
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Whether to log requests to database
    #[serde(default = "default_true")]
    pub log_requests: bool,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            log_requests: true,
        }
    }
}

impl Config {
    /// Load configuration from a TOML file.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| ConfigError::Io {
            path: path.as_ref().display().to_string(),
            source: e,
        })?;

        Self::parse_str(&content)
    }

    /// Parse configuration from a TOML string.
    pub fn parse_str(content: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(content).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.providers.is_empty() {
            tracing::warn!("No providers configured - proxy will reject all requests");
        }

        for provider in &self.providers {
            if provider.url.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "Provider '{}' has empty URL",
                    provider.name
                )));
            }
        }

        Ok(())
    }

    /// Get database config with defaults.
    pub fn database(&self) -> DatabaseConfig {
        self.database.clone().unwrap_or_default()
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Configuration validation error: {0}")]
    Validation(String),

    #[error("Environment variable '{var}' not set for provider '{provider}': {message}")]
    EnvVar {
        var: String,
        provider: String,
        message: String,
    },
}

/// Raw provider config deserialized directly from TOML.
/// api_key is `Option<String>` so it may contain `${VAR}` references not yet expanded.
#[derive(Deserialize)]
pub struct RawProviderConfig {
    name: String,
    url: String,
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    input_rate: u64,
    #[serde(default)]
    output_rate: u64,
    #[serde(default)]
    base_fee: u64,
}

/// Raw configuration deserialized directly from TOML.
/// Provider api_key values may contain `${VAR}` references not yet expanded.
#[derive(Deserialize)]
pub struct RawConfig {
    server: ServerConfig,
    database: Option<DatabaseConfig>,
    #[serde(default)]
    providers: Vec<RawProviderConfig>,
    #[serde(default)]
    policies: PoliciesConfig,
    #[serde(default)]
    logging: LoggingConfig,
}

/// Expand all `${VAR}` references in a string using a custom lookup function.
///
/// The closure-based design makes this testable without touching global env state.
/// Supports multiple `${VAR}` in one string (e.g., `${SCHEME}://${HOST}/v1`).
/// Fails on first missing variable, unclosed `${`, or empty variable name.
fn expand_env_vars_with<F>(
    input: &str,
    provider_name: &str,
    lookup: F,
) -> Result<String, ConfigError>
where
    F: Fn(&str) -> Option<String>,
{
    if !input.contains("${") {
        return Ok(input.to_string());
    }

    let mut result = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];

        let end = after.find('}').ok_or_else(|| ConfigError::EnvVar {
            var: "<unclosed>".to_string(),
            provider: provider_name.to_string(),
            message: format!("Unclosed '${{' in config value: {}", input),
        })?;

        let var_name = &after[..end];
        if var_name.is_empty() {
            return Err(ConfigError::EnvVar {
                var: "".to_string(),
                provider: provider_name.to_string(),
                message: "Empty variable name in '${}' reference".to_string(),
            });
        }

        let value = lookup(var_name).ok_or_else(|| ConfigError::EnvVar {
            var: var_name.to_string(),
            provider: provider_name.to_string(),
            message: format!(
                "Environment variable '{}' is not set (referenced in provider '{}')",
                var_name, provider_name
            ),
        })?;

        result.push_str(&value);
        rest = &after[end + 1..];
    }

    result.push_str(rest);
    Ok(result)
}

/// Expand all `${VAR}` references in a string using real environment variables.
fn expand_env_vars(input: &str, provider_name: &str) -> Result<String, ConfigError> {
    expand_env_vars_with(input, provider_name, |name| std::env::var(name).ok())
}

/// Derive the convention-based env var name for a provider.
///
/// Transforms provider name to `ARBSTR_<UPPER_SNAKE_NAME>_API_KEY`:
/// - "alpha" -> "ARBSTR_ALPHA_API_KEY"
/// - "provider-beta" -> "ARBSTR_PROVIDER_BETA_API_KEY"
/// - "my_service" -> "ARBSTR_MY_SERVICE_API_KEY"
pub fn convention_env_var_name(provider_name: &str) -> String {
    let upper_snake = provider_name.to_uppercase().replace(['-', ' '], "_");
    format!("ARBSTR_{}_API_KEY", upper_snake)
}

/// Try convention-based env var lookup for a provider's API key.
///
/// Returns `Some((var_name, value))` if `ARBSTR_<NAME>_API_KEY` is set.
fn convention_key_lookup(provider_name: &str) -> Option<(String, String)> {
    let var_name = convention_env_var_name(provider_name);
    std::env::var(&var_name).ok().map(|value| (var_name, value))
}

impl Config {
    /// Convert raw (deserialized) config to final config with env var expansion.
    ///
    /// For each provider:
    /// - If `api_key` contains `${VAR}`: expand from environment, source = `EnvExpanded`
    /// - If `api_key` is a literal string: wrap directly, source = `Literal`
    /// - If `api_key` is absent: try convention lookup (`ARBSTR_<NAME>_API_KEY`),
    ///   source = `Convention(var_name)` or `KeySource::None`
    pub fn from_raw(raw: RawConfig) -> Result<(Self, Vec<(String, KeySource)>), ConfigError> {
        let mut providers = Vec::with_capacity(raw.providers.len());
        let mut key_sources = Vec::with_capacity(raw.providers.len());

        for rp in raw.providers {
            let (api_key, source) = match rp.api_key {
                Some(ref raw_key) if raw_key.contains("${") => {
                    let expanded = expand_env_vars(raw_key, &rp.name)?;
                    (Some(ApiKey::from(expanded)), KeySource::EnvExpanded)
                }
                Some(ref raw_key) => (Some(ApiKey::from(raw_key.as_str())), KeySource::Literal),
                None => match convention_key_lookup(&rp.name) {
                    Some((var_name, value)) => {
                        (Some(ApiKey::from(value)), KeySource::Convention(var_name))
                    }
                    None => (None, KeySource::None),
                },
            };

            key_sources.push((rp.name.clone(), source));

            providers.push(ProviderConfig {
                name: rp.name,
                url: rp.url,
                api_key,
                models: rp.models,
                input_rate: rp.input_rate,
                output_rate: rp.output_rate,
                base_fee: rp.base_fee,
            });
        }

        let config = Config {
            server: raw.server,
            database: raw.database,
            providers,
            policies: raw.policies,
            logging: raw.logging,
        };

        Ok((config, key_sources))
    }

    /// Load configuration from a TOML file with environment variable expansion.
    ///
    /// This is the env-var-aware entry point. It:
    /// 1. Reads the file
    /// 2. Parses as `RawConfig` (api_key as plain String)
    /// 3. Expands `${VAR}` references and applies convention lookup
    /// 4. Validates the resulting config
    ///
    /// Returns the config and per-provider key source information.
    pub fn from_file_with_env(
        path: impl AsRef<Path>,
    ) -> Result<(Self, Vec<(String, KeySource)>), ConfigError> {
        let content = std::fs::read_to_string(path.as_ref()).map_err(|e| ConfigError::Io {
            path: path.as_ref().display().to_string(),
            source: e,
        })?;

        let raw: RawConfig = toml::from_str(&content).map_err(ConfigError::Parse)?;
        let (config, key_sources) = Self::from_raw(raw)?;
        config.validate()?;

        Ok((config, key_sources))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
            [server]
            listen = "127.0.0.1:9000"
        "#;

        let config = Config::parse_str(toml).unwrap();
        assert_eq!(config.server.listen, "127.0.0.1:9000");
        assert!(config.providers.is_empty());
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
            [server]
            listen = "0.0.0.0:8080"

            [database]
            path = "./test.db"

            [[providers]]
            name = "test-provider"
            url = "https://example.com/v1"
            models = ["gpt-4o", "claude-3.5-sonnet"]
            input_rate = 10
            output_rate = 30
            base_fee = 1

            [policies]
            default_strategy = "cheapest"

            [[policies.rules]]
            name = "code"
            allowed_models = ["gpt-4o"]
            strategy = "lowest_cost"
            max_sats_per_1k_output = 50
            keywords = ["code", "function"]

            [logging]
            level = "debug"
            log_requests = true
        "#;

        let config = Config::parse_str(toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "test-provider");
        assert_eq!(config.providers[0].input_rate, 10);
        assert_eq!(config.policies.rules.len(), 1);
        assert_eq!(config.policies.rules[0].name, "code");
    }

    #[test]
    fn test_api_key_debug_redaction() {
        let key = ApiKey::from("super-secret-cashu-token");
        let debug_output = format!("{:?}", key);
        assert_eq!(debug_output, "[REDACTED]");
        assert!(!debug_output.contains("super-secret"));
    }

    #[test]
    fn test_api_key_display_redaction() {
        let key = ApiKey::from("super-secret-cashu-token");
        let display_output = format!("{}", key);
        assert_eq!(display_output, "[REDACTED]");
        assert!(!display_output.contains("super-secret"));
    }

    #[test]
    fn test_api_key_serialize_redaction() {
        let key = ApiKey::from("real-secret-value");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, "\"[REDACTED]\"");
        assert!(!json.contains("real-secret"));
    }

    #[test]
    fn test_api_key_deserialize_from_string() {
        let key: ApiKey = serde_json::from_str("\"my-secret-key\"").unwrap();
        assert_eq!(key.expose_secret(), "my-secret-key");
    }

    #[test]
    fn test_api_key_expose_secret() {
        let key = ApiKey::from("the-actual-value");
        assert_eq!(key.expose_secret(), "the-actual-value");
    }

    #[test]
    fn test_provider_config_debug_redaction() {
        let config = ProviderConfig {
            name: "test".to_string(),
            url: "https://example.com/v1".to_string(),
            api_key: Some(ApiKey::from("cashuABCD1234secret")),
            models: vec![],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
        };
        let debug_output = format!("{:?}", config);
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output should contain [REDACTED]"
        );
        assert!(
            !debug_output.contains("cashuABCD1234secret"),
            "Debug output must not contain actual key"
        );
    }

    #[test]
    fn test_api_key_toml_deserialization() {
        let toml = r#"
            [server]
            listen = "127.0.0.1:9000"

            [[providers]]
            name = "test-provider"
            url = "https://example.com/v1"
            api_key = "cashuABCD1234secret"
            models = ["gpt-4o"]
            input_rate = 10
            output_rate = 30
            base_fee = 1
        "#;

        let config = Config::parse_str(toml).unwrap();
        assert_eq!(
            config.providers[0]
                .api_key
                .as_ref()
                .unwrap()
                .expose_secret(),
            "cashuABCD1234secret"
        );
        // Verify Debug doesn't leak
        let debug = format!("{:?}", config.providers[0]);
        assert!(!debug.contains("cashuABCD1234secret"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn test_provider_config_without_api_key() {
        let toml = r#"
            [server]
            listen = "127.0.0.1:9000"

            [[providers]]
            name = "no-key-provider"
            url = "https://example.com/v1"
            models = ["gpt-4o"]
        "#;

        let config = Config::parse_str(toml).unwrap();
        assert!(config.providers[0].api_key.is_none());
    }

    // ── Expansion tests (using expand_env_vars_with, no global env state) ──

    #[test]
    fn test_expand_single_var() {
        let lookup = |name: &str| match name {
            "MY_KEY" => Some("cashuABCD".to_string()),
            _ => None,
        };
        let result = expand_env_vars_with("${MY_KEY}", "test", lookup).unwrap();
        assert_eq!(result, "cashuABCD");
    }

    #[test]
    fn test_expand_multiple_vars() {
        let lookup = |name: &str| match name {
            "SCHEME" => Some("https".to_string()),
            "HOST" => Some("example.com".to_string()),
            _ => None,
        };
        let result = expand_env_vars_with("${SCHEME}://${HOST}/v1", "test", lookup).unwrap();
        assert_eq!(result, "https://example.com/v1");
    }

    #[test]
    fn test_expand_no_vars_passthrough() {
        let lookup = |_: &str| -> Option<String> { panic!("should not be called") };
        let result = expand_env_vars_with("literal-value", "test", lookup).unwrap();
        assert_eq!(result, "literal-value");
    }

    #[test]
    fn test_expand_mixed_literal_and_var() {
        let lookup = |name: &str| match name {
            "KEY" => Some("resolved".to_string()),
            _ => None,
        };
        let result = expand_env_vars_with("prefix-${KEY}-suffix", "test", lookup).unwrap();
        assert_eq!(result, "prefix-resolved-suffix");
    }

    #[test]
    fn test_expand_missing_var_fails() {
        let lookup = |_: &str| None;
        let result = expand_env_vars_with("${MISSING}", "provider-alpha", lookup);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("MISSING"), "Error should name the variable");
        assert!(
            err.contains("provider-alpha"),
            "Error should name the provider"
        );
    }

    #[test]
    fn test_expand_unclosed_brace_fails() {
        let lookup = |_: &str| -> Option<String> { panic!("should not be called") };
        let result = expand_env_vars_with("${UNCLOSED", "test", lookup);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("unclosed"),
            "Error should mention unclosed brace"
        );
    }

    #[test]
    fn test_expand_empty_var_name_fails() {
        let lookup = |_: &str| -> Option<String> { panic!("should not be called") };
        let result = expand_env_vars_with("${}", "test", lookup);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string().to_lowercase();
        assert!(
            err.contains("empty"),
            "Error should mention empty variable name"
        );
    }

    #[test]
    fn test_expand_dollar_without_brace_passthrough() {
        let lookup = |_: &str| -> Option<String> { panic!("should not be called") };
        let result = expand_env_vars_with("$NOT_A_VAR", "test", lookup).unwrap();
        assert_eq!(result, "$NOT_A_VAR");
    }

    // ── Convention name tests ──

    #[test]
    fn test_convention_env_var_name_simple() {
        assert_eq!(convention_env_var_name("alpha"), "ARBSTR_ALPHA_API_KEY");
    }

    #[test]
    fn test_convention_env_var_name_hyphen() {
        assert_eq!(
            convention_env_var_name("provider-beta"),
            "ARBSTR_PROVIDER_BETA_API_KEY"
        );
    }

    #[test]
    fn test_convention_env_var_name_underscore() {
        assert_eq!(
            convention_env_var_name("my_service"),
            "ARBSTR_MY_SERVICE_API_KEY"
        );
    }

    // ── from_raw integration tests ──

    /// Helper to construct a minimal RawConfig with a single provider.
    fn make_raw_config(provider_name: &str, api_key: Option<String>) -> RawConfig {
        RawConfig {
            server: ServerConfig {
                listen: "127.0.0.1:9000".to_string(),
            },
            database: None,
            providers: vec![RawProviderConfig {
                name: provider_name.to_string(),
                url: "https://example.com/v1".to_string(),
                api_key,
                models: vec![],
                input_rate: 0,
                output_rate: 0,
                base_fee: 0,
            }],
            policies: PoliciesConfig::default(),
            logging: LoggingConfig::default(),
        }
    }

    #[test]
    fn test_from_raw_literal_key() {
        let raw = make_raw_config("test-literal", Some("literal-key-value".to_string()));
        let (config, key_sources) = Config::from_raw(raw).unwrap();

        assert_eq!(key_sources.len(), 1);
        assert_eq!(key_sources[0].0, "test-literal");
        assert_eq!(key_sources[0].1, KeySource::Literal);
        assert_eq!(
            config.providers[0]
                .api_key
                .as_ref()
                .unwrap()
                .expose_secret(),
            "literal-key-value"
        );
    }

    #[test]
    fn test_from_raw_env_expanded_key() {
        // Use a unique env var name to avoid parallel test interference
        let var_name = "TEST_06_01_EXPAND_KEY";
        let var_value = "cashu-expanded-token-abc123";
        unsafe { std::env::set_var(var_name, var_value) };

        let raw = make_raw_config("test-env-expand", Some(format!("${{{}}}", var_name)));
        let (config, key_sources) = Config::from_raw(raw).unwrap();

        assert_eq!(key_sources[0].1, KeySource::EnvExpanded);
        assert_eq!(
            config.providers[0]
                .api_key
                .as_ref()
                .unwrap()
                .expose_secret(),
            var_value
        );

        unsafe { std::env::remove_var(var_name) };
    }

    #[test]
    fn test_from_raw_convention_key() {
        // Use a unique provider name that maps to a unique env var
        let provider_name = "test-conv-0601";
        let var_name = convention_env_var_name(provider_name);
        let var_value = "cashu-convention-token-xyz789";
        unsafe { std::env::set_var(&var_name, var_value) };

        let raw = make_raw_config(provider_name, None);
        let (config, key_sources) = Config::from_raw(raw).unwrap();

        assert_eq!(key_sources[0].1, KeySource::Convention(var_name.clone()));
        assert_eq!(
            config.providers[0]
                .api_key
                .as_ref()
                .unwrap()
                .expose_secret(),
            var_value
        );

        unsafe { std::env::remove_var(&var_name) };
    }

    #[test]
    fn test_from_raw_no_key() {
        // Ensure no convention env var is set for this provider
        let provider_name = "test-nokey-0601-unique";
        let var_name = convention_env_var_name(provider_name);
        unsafe { std::env::remove_var(&var_name) };

        let raw = make_raw_config(provider_name, None);
        let (config, key_sources) = Config::from_raw(raw).unwrap();

        assert_eq!(key_sources[0].1, KeySource::None);
        assert!(config.providers[0].api_key.is_none());
    }

    #[test]
    fn test_from_raw_missing_env_var_fails() {
        // Ensure this env var is definitely not set
        let var_name = "TEST_06_01_DEFINITELY_MISSING";
        unsafe { std::env::remove_var(var_name) };

        let raw = make_raw_config("test-missing-env", Some(format!("${{{}}}", var_name)));
        let result = Config::from_raw(raw);

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains(var_name),
            "Error should name the variable: {}",
            err
        );
        assert!(
            err.contains("test-missing-env"),
            "Error should name the provider: {}",
            err
        );
    }
}
