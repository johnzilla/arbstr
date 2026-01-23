//! arbstr - Intelligent LLM routing and cost arbitrage for Routstr
//!
//! A local proxy that optimizes LLM costs by routing requests to the
//! cheapest provider while respecting quality constraints.

use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
        Commands::Serve { config, listen } => {
            tracing::info!("Starting arbstr proxy server");
            tracing::info!(config = %config, "Loading configuration");

            if let Some(addr) = listen {
                tracing::info!(listen = %addr, "Override listen address");
            }

            // TODO: Load config
            // TODO: Initialize database
            // TODO: Start HTTP server

            tracing::warn!("Server not yet implemented - this is a skeleton");
            Ok(())
        }

        Commands::Check { config } => {
            tracing::info!(config = %config, "Checking configuration");
            // TODO: Parse and validate config
            tracing::warn!("Config validation not yet implemented");
            Ok(())
        }

        Commands::Providers { config } => {
            tracing::info!(config = %config, "Listing providers");
            // TODO: Load config and display providers
            tracing::warn!("Provider listing not yet implemented");
            Ok(())
        }
    }
}
