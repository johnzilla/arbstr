//! HTTP proxy server module.
//!
//! This module provides the OpenAI-compatible HTTP API that accepts
//! requests and forwards them to selected providers.

mod handlers;
pub mod retry;
mod server;
pub mod stream;
pub mod types;

pub use server::{run_server, AppState, RequestId};
pub use types::{
    ensure_stream_options, ChatCompletionRequest, ChatCompletionResponse, Message, StreamOptions,
};
