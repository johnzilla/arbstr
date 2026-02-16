//! SQLite storage for request logging and metrics.

pub mod logging;

pub use logging::{spawn_stream_completion_update, spawn_usage_update, update_stream_completion, update_usage, RequestLog};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::str::FromStr;

/// Initialize the SQLite connection pool and run migrations.
///
/// The database file is created automatically if it doesn't exist.
/// WAL journal mode is used for concurrent read/write performance.
pub async fn init_pool(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path))?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;

    // Apply embedded migrations
    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}
