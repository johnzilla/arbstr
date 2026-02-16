//! arbstr - Intelligent LLM routing and cost arbitrage for Routstr
//!
//! A local proxy that optimizes LLM costs by routing requests to the
//! cheapest provider while respecting quality constraints.

use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use arbstr::config::{Config, KeySource};
use arbstr::proxy::run_server;

#[derive(Parser)]
#[command(name = "arbstr")]
#[command(about = "Intelligent LLM routing and cost arbitrage for Routstr")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the proxy server
    Serve {
        /// Path to configuration file
        #[arg(short, long, default_value = "config.toml")]
        config: String,

        /// Override listen address
        #[arg(short, long)]
        listen: Option<String>,

        /// Run with a mock provider for testing (no real API calls)
        #[arg(long)]
        mock: bool,
    },

    /// Validate configuration file
    Check {
        /// Path to configuration file
        #[arg(short, long, default_value = "config.toml")]
        config: String,
    },

    /// Show configured providers and their rates
    Providers {
        /// Path to configuration file
        #[arg(short, long, default_value = "config.toml")]
        config: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "arbstr=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve {
            config: config_path,
            listen,
            mock,
        } => {
            tracing::info!("Starting arbstr proxy server");

            let (mut config, key_sources) = if mock {
                tracing::info!("Using mock configuration");
                (mock_config(), vec![])
            } else {
                tracing::info!(config = %config_path, "Loading configuration");
                let result = Config::from_file_with_env(&config_path)?;

                // RED-01: Warn if config file permissions are too open
                if let Some((path, mode)) =
                    arbstr::config::check_file_permissions(std::path::Path::new(&config_path))
                {
                    tracing::warn!(
                        file = %path,
                        permissions = format_args!("{:04o}", mode),
                        "Config file has permissions more open than 0600. Consider: chmod 600 {}",
                        path
                    );
                }

                result
            };

            // Override listen address if specified
            if let Some(addr) = listen {
                config.server.listen = addr;
            }

            tracing::info!(
                listen = %config.server.listen,
                providers = %config.providers.len(),
                "Configuration loaded"
            );

            for (provider_name, source) in &key_sources {
                match source {
                    KeySource::Literal => {
                        tracing::info!(provider = %provider_name, "key from config-literal");
                        tracing::warn!(
                            provider = %provider_name,
                            "Plaintext API key in config file. Consider using environment variables: \
                             set {} or use api_key = \"${{{}}}\"",
                            arbstr::config::convention_env_var_name(provider_name),
                            arbstr::config::convention_env_var_name(provider_name)
                        );
                    }
                    KeySource::EnvExpanded => {
                        tracing::info!(provider = %provider_name, "key from env-expanded")
                    }
                    KeySource::Convention(var) => {
                        tracing::info!(provider = %provider_name, env_var = %var, "key from convention")
                    }
                    KeySource::None => {
                        tracing::warn!(provider = %provider_name, "no api key available")
                    }
                }
            }

            run_server(config).await?;
            Ok(())
        }

        Commands::Check {
            config: config_path,
        } => {
            match Config::from_file_with_env(&config_path) {
                Ok((config, key_sources)) => {
                    println!("Configuration is valid!");
                    println!("  Listen: {}", config.server.listen);
                    println!("  Providers: {}", config.providers.len());
                    println!("  Policy rules: {}", config.policies.rules.len());

                    // RED-01: Check config file permissions
                    if let Some((path, mode)) =
                        arbstr::config::check_file_permissions(std::path::Path::new(&config_path))
                    {
                        println!();
                        println!("  WARNING: Config file '{}' has permissions {:04o} (more open than 0600)", path, mode);
                        println!("  Consider: chmod 600 {}", path);
                    }

                    println!();
                    println!("Provider key status:");
                    for (name, source) in &key_sources {
                        match source {
                            KeySource::Literal => {
                                let conv_var = arbstr::config::convention_env_var_name(name);
                                println!("  {}: key from config-literal", name);
                                println!("    WARNING: Plaintext key. Consider: set {} or use api_key = \"${{{}}}\"", conv_var, conv_var);
                            }
                            KeySource::EnvExpanded => {
                                println!("  {}: key from env-expanded", name)
                            }
                            KeySource::Convention(var) => {
                                println!("  {}: key from convention ({})", name, var)
                            }
                            KeySource::None => {
                                let expected = arbstr::config::convention_env_var_name(name);
                                println!(
                                    "  {}: no key (set {} or add api_key to config)",
                                    name, expected
                                );
                            }
                        }
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Configuration error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Providers {
            config: config_path,
        } => {
            let (config, _key_sources) = Config::from_file_with_env(&config_path)?;

            if config.providers.is_empty() {
                println!("No providers configured.");
            } else {
                println!("Configured providers:\n");
                for provider in &config.providers {
                    println!("  {} ({})", provider.name, provider.url);
                    if !provider.models.is_empty() {
                        println!("    Models: {}", provider.models.join(", "));
                    }
                    println!(
                        "    Rates: {} sats/1k input, {} sats/1k output",
                        provider.input_rate, provider.output_rate
                    );
                    if provider.base_fee > 0 {
                        println!("    Base fee: {} sats", provider.base_fee);
                    }
                    if let Some(ref api_key) = provider.api_key {
                        println!("    Key: {}", api_key.masked_prefix());
                    }
                    println!();
                }
            }
            Ok(())
        }
    }
}

/// Create a mock configuration for testing without real providers.
fn mock_config() -> Config {
    use arbstr::config::*;

    Config {
        server: ServerConfig {
            listen: "127.0.0.1:8080".to_string(),
        },
        database: Some(DatabaseConfig {
            path: ":memory:".to_string(),
        }),
        providers: vec![
            ProviderConfig {
                name: "mock-cheap".to_string(),
                url: "http://localhost:9999/v1".to_string(), // Won't be called in mock mode
                api_key: Some(ApiKey::from("mock-test-key-cheap")),
                models: vec![
                    "gpt-4o".to_string(),
                    "gpt-4o-mini".to_string(),
                    "claude-3.5-sonnet".to_string(),
                ],
                input_rate: 5,
                output_rate: 15,
                base_fee: 0,
            },
            ProviderConfig {
                name: "mock-expensive".to_string(),
                url: "http://localhost:9998/v1".to_string(),
                api_key: Some(ApiKey::from("mock-test-key-expensive")),
                models: vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()],
                input_rate: 10,
                output_rate: 30,
                base_fee: 1,
            },
        ],
        policies: PoliciesConfig {
            default_strategy: "cheapest".to_string(),
            rules: vec![PolicyRule {
                name: "code".to_string(),
                allowed_models: vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()],
                strategy: "lowest_cost".to_string(),
                max_sats_per_1k_output: Some(50),
                keywords: vec![
                    "code".to_string(),
                    "function".to_string(),
                    "implement".to_string(),
                ],
            }],
        },
        logging: LoggingConfig {
            level: "debug".to_string(),
            log_requests: true,
        },
    }
}
