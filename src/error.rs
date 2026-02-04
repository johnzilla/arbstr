//! Error types for arbstr.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Result type alias for arbstr operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Main error type for arbstr.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Configuration error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("No providers available for model '{model}'")]
    NoProviders { model: String },

    #[error("No providers match policy constraints")]
    NoPolicyMatch,

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Upstream request failed: {0}")]
    Upstream(#[from] reqwest::Error),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Error::Config(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            Error::NoProviders { .. } => (StatusCode::BAD_REQUEST, self.to_string()),
            Error::NoPolicyMatch => (StatusCode::BAD_REQUEST, self.to_string()),
            Error::Provider(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Error::Upstream(_) => (StatusCode::BAD_GATEWAY, self.to_string()),
            Error::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            Error::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
            Error::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };

        // Return OpenAI-compatible error format
        let body = serde_json::json!({
            "error": {
                "message": message,
                "type": "arbstr_error",
                "code": status.as_u16()
            }
        });

        (status, axum::Json(body)).into_response()
    }
}
