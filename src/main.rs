//! arbstr - Intelligent LLM routing and cost arbitrage for Routstr
//!
//! A local proxy that optimizes LLM costs by routing requests to the
//! cheapest provider while respecting quality constraints.

use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use arbstr::config::Config;
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

            let mut config = if mock {
                tracing::info!("Using mock configuration");
                mock_config()
            } else {
                tracing::info!(config = %config_path, "Loading configuration");
                Config::from_file(&config_path)?
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

            run_server(config).await?;
            Ok(())
        }

        Commands::Check { config: config_path } => {
            match Config::from_file(&config_path) {
                Ok(config) => {
                    println!("Configuration is valid!");
                    println!("  Listen: {}", config.server.listen);
                    println!("  Providers: {}", config.providers.len());
                    println!("  Policy rules: {}", config.policies.rules.len());
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Configuration error: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Providers { config: config_path } => {
            let config = Config::from_file(&config_path)?;

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
