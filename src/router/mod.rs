//! Router module for provider selection.
//!
//! This module handles selecting the optimal provider based on:
//! - Model availability
//! - Cost (input/output rates)
//! - Policy constraints

mod complexity;
mod selector;

pub use complexity::{score_complexity, score_to_max_tier};
pub use selector::{actual_cost_sats, Router, SelectedProvider};
