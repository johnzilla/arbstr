//! arbstr - Intelligent LLM routing and cost arbitrage for Routstr
//!
//! This library provides the core functionality for the arbstr proxy,
//! including configuration, routing, and provider management.

pub mod config;
pub mod error;
pub mod proxy;
pub mod router;

pub use config::Config;
pub use error::{Error, Result};
