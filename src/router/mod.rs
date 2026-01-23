//! Router module for provider selection.
//!
//! This module handles selecting the optimal provider based on:
//! - Model availability
//! - Cost (input/output rates)
//! - Policy constraints

mod selector;

pub use selector::{Router, SelectedProvider};
