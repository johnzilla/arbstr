//! Configuration parsing and validation for arbstr.

use serde::Deserialize;
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

/// Provider configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    /// Unique name for this provider
    pub name: String,
    /// Base URL for the provider's API (e.g., "https://api.routstr.com/v1")
    pub url: String,
    /// Optional API key or Cashu token
    pub api_key: Option<String>,
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

        Self::from_str(&content)
    }

    /// Parse configuration from a TOML string.
    pub fn from_str(content: &str) -> Result<Self, ConfigError> {
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

        let config = Config::from_str(toml).unwrap();
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

        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.providers[0].name, "test-provider");
        assert_eq!(config.providers[0].input_rate, 10);
        assert_eq!(config.policies.rules.len(), 1);
        assert_eq!(config.policies.rules[0].name, "code");
    }
}
