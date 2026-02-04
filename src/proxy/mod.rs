//! HTTP proxy server module.
//!
//! This module provides the OpenAI-compatible HTTP API that accepts
//! requests and forwards them to selected providers.

mod handlers;
mod server;
mod types;

pub use server::{run_server, AppState, RequestId};
pub use types::{ChatCompletionRequest, ChatCompletionResponse, Message};
