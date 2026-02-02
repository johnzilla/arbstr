# Coding Conventions

**Analysis Date:** 2026-02-02

## Naming Patterns

**Files:**
- Module names are lowercase with underscores: `server.rs`, `selector.rs`, `handlers.rs`
- Files map to module structure: `src/proxy/server.rs` → `mod server`
- No special suffixes for test files; tests are in `#[cfg(test)]` blocks within modules

**Functions:**
- Snake_case throughout: `run_server`, `create_router`, `select_cheapest`, `list_models`, `apply_policy_constraints`
- Handler functions use descriptive action verbs: `chat_completions`, `list_models`, `health`, `list_providers`
- Private helper methods use snake_case: `find_policy`, `select_first`, `user_prompt`

**Variables:**
- Snake_case for all local and module-level variables: `listen_addr`, `provider_router`, `http_client`, `policy_name`, `upstream_url`
- Field names in structs use snake_case: `input_rate`, `output_rate`, `base_fee`, `max_sats_per_1k_output`
- Loop variables and iterators are concise: `p` for provider, `m` for model, `kw` for keyword

**Types:**
- PascalCase for all public types: `Config`, `Router`, `AppState`, `Error`, `SelectedProvider`, `ProviderConfig`
- Enum variants use PascalCase: `NoProviders`, `NoPolicyMatch`, `BadRequest`, `Internal`
- Type aliases use `Result<T>` pattern: `pub type Result<T> = std::result::Result<T, Error>`

**Constants:**
- SCREAMING_SNAKE_CASE: `ARBSTR_POLICY_HEADER = "x-arbstr-policy"`

## Code Style

**Formatting:**
- Standard Rust conventions (implicitly cargo fmt compatible)
- 4-space indentation (standard Rust)
- Lines typically under 100 characters; longer lines allowed for readability
- Module documentation at file level with `//!` comments

**Linting:**
- Uses `clippy` with default warnings treated as errors (per CLAUDE.md: `cargo clippy -- -D warnings`)
- Follows standard Rust idioms and best practices
- No special clippy.toml configuration detected; relies on defaults

## Import Organization

**Order:**
1. Standard library imports (`std::`)
2. External crate imports (tokio, axum, serde, etc.)
3. Internal crate imports (`crate::`)
4. Re-exports in `pub use` statements

**Pattern in files:**
```rust
use axum::{
    routing::{get, post},
    Router,
};
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tower_http::trace::TraceLayer;

use super::handlers;
use crate::config::Config;
use crate::router::Router as ProviderRouter;
```

**Path Aliases:**
- No aliases defined in `Cargo.toml` workspace configuration
- Uses module-level re-exports via `pub use` in `mod.rs` files
- Example: `src/proxy/mod.rs` re-exports: `pub use server::{run_server, AppState}`

## Error Handling

**Patterns:**
- Custom error type `Error` defined in `src/error.rs` using `thiserror`
- All result types use `Result<T>` alias: `pub type Result<T> = std::result::Result<T, Error>`
- Error variants are explicit enum members with context fields
- Example pattern:
  ```rust
  #[error("No providers available for model '{model}'")]
  NoProviders { model: String },
  ```
- Errors implement `IntoResponse` for axum HTTP integration
- OpenAI-compatible error responses returned as JSON with `error` object containing `message`, `type`, and `code`

**Propagation:**
- Uses `?` operator for immediate propagation
- Wraps external errors (reqwest, toml parsing, I/O) with context
- Example: `Config::from_file` wraps `std::io::Error` as `ConfigError::Io` with path context
- Handler functions return `Result<Response, Error>` with automatic response conversion

## Logging

**Framework:** `tracing` with `tracing_subscriber` for output

**Patterns:**
- Structured logging with field names: `tracing::info!(address = %listen_addr, "message")`
- `%` formatter for Display trait, `?` for Debug trait
- Conditional logging at appropriate levels:
  - `info!` for startup messages and major events
  - `debug!` for policy matching details (`find_policy` method)
  - `warn!` for policy violations
  - `error!` for failures with full context
- Example:
  ```rust
  tracing::info!(
      model = %request.model,
      policy = ?policy_name,
      stream = ?request.stream,
      "Received chat completion request"
  );
  ```
- Log level controlled via `RUST_LOG` environment variable (default: `arbstr=info,tower_http=info`)

## Comments

**When to Comment:**
- Module-level `//!` documentation for all public modules and files
- Doc comments `///` for public functions and types with description of purpose
- Inline comments `//` sparingly; code is generally self-documenting
- TODO comments for incomplete features: `// TODO: Log to database for cost tracking`

**JSDoc/TSDoc:**
- Rust uses `///` doc comments instead of JSDoc
- Example from `src/router/selector.rs`:
  ```rust
  /// Select the best provider for a request.
  ///
  /// # Arguments
  /// * `model` - The requested model name
  /// * `policy_name` - Optional policy name from X-Arbstr-Policy header
  /// * `prompt` - The user's prompt (for heuristic matching)
  pub fn select(
      &self,
      model: &str,
      policy_name: Option<&str>,
      prompt: Option<&str>,
  ) -> Result<SelectedProvider>
  ```
- Section headers like `# Arguments`, `# Returns` are standard

## Function Design

**Size:**
- Most functions 20-50 lines
- Router logic split into focused helpers: `find_policy`, `apply_policy_constraints`, `select_cheapest`, `select_first`
- Handlers contain all request/response handling within one function for clarity

**Parameters:**
- Prefer specific types over generic where clarity matters
- Use `Option<T>` for optional parameters rather than `None` values
- &str for string parameters that don't need ownership
- Example: `fn select(&self, model: &str, policy_name: Option<&str>, prompt: Option<&str>)`

**Return Values:**
- Always explicit `Result<T>` for fallible operations
- Single values wrapped in appropriate types: `Option<&ProviderConfig>`, `Vec<SelectedProvider>`
- Implementations of `IntoResponse` trait for axum handlers

## Module Design

**Exports:**
- Explicit `pub use` re-exports in `mod.rs` files
- Example: `src/proxy/mod.rs` re-exports only public API: `pub use server::{run_server, AppState}`
- Internal modules (handlers, types) not exposed through crate root

**Barrel Files:**
- `src/lib.rs` re-exports only stable public APIs:
  ```rust
  pub mod config;
  pub mod error;
  pub mod proxy;
  pub mod router;

  pub use config::Config;
  pub use error::{Error, Result};
  ```
- `src/main.rs` imports from lib: `use arbstr::proxy::run_server`

**Organization:**
- Code organized by concern: `proxy/` (HTTP layer), `router/` (selection logic), `config/` (parsing), `error/` (types)
- Middleware and cross-cutting concerns (logging, tracing) integrated via axum layers
- Test modules inline within implementation files with `#[cfg(test)]` guard

---

*Convention analysis: 2026-02-02*
