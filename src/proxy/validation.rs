//! Shared filter validation for stats and logs endpoints.

use crate::config::Config;
use crate::error::Error;
use crate::storage;
use sqlx::SqlitePool;

/// Validate that a model exists in config or database, returning 404 if not found.
pub async fn validate_model_filter(
    config: &Config,
    pool: &SqlitePool,
    model: &str,
) -> Result<(), Error> {
    let in_config = config
        .providers
        .iter()
        .any(|p| p.models.iter().any(|m| m.eq_ignore_ascii_case(model)));
    if !in_config {
        let in_db = storage::stats::exists_in_db(pool, "model", model).await?;
        if !in_db {
            return Err(Error::NotFound(format!("Model '{}' not found", model)));
        }
    }
    Ok(())
}

/// Validate that a provider exists in config or database, returning 404 if not found.
pub async fn validate_provider_filter(
    config: &Config,
    pool: &SqlitePool,
    provider: &str,
) -> Result<(), Error> {
    let in_config = config
        .providers
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(provider));
    if !in_config {
        let in_db = storage::stats::exists_in_db(pool, "provider", provider).await?;
        if !in_db {
            return Err(Error::NotFound(format!(
                "Provider '{}' not found",
                provider
            )));
        }
    }
    Ok(())
}
