//! HTTP proxy server module.
//!
//! This module provides the OpenAI-compatible HTTP API that accepts
//! requests and forwards them to selected providers.

mod handlers;
pub mod logs;
pub mod retry;
mod server;
pub mod stats;
pub mod stream;
pub mod types;

pub use server::{create_router, run_server, AppState, RequestId};
pub mod circuit_breaker;
pub use stream::{wrap_sse_stream, StreamResult, StreamResultHandle, StreamUsage};
pub use types::{
    ensure_stream_options, ChatCompletionRequest, ChatCompletionResponse, Message, StreamOptions,
};
