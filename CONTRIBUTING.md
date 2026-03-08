# Contributing to arbstr

Thanks for your interest in contributing to arbstr! This document covers everything you need to get started.

## Getting Started

1. Clone the repo:
   ```bash
   git clone https://github.com/johnzilla/arbstr.git
   cd arbstr
   ```

2. Install the Rust toolchain (stable): [rustup.rs](https://rustup.rs)

3. Build:
   ```bash
   cargo build
   ```

4. Run tests:
   ```bash
   cargo test
   ```

5. Run with mock providers (no real API calls needed):
   ```bash
   cargo run -- serve --mock
   ```

## Development Workflow

- Format code before committing: `cargo fmt`
- Run clippy: `cargo clippy -- -D warnings`
- Run the full check before pushing:
  ```bash
  cargo fmt && cargo clippy -- -D warnings && cargo test
  ```
- Use debug logging for development: `RUST_LOG=arbstr=debug cargo run -- serve --mock`

## Code Style

Follow the conventions documented in [DEVELOPMENT.md](./DEVELOPMENT.md#code-conventions):

- Use `thiserror` for error types
- Async everywhere (no blocking in async context)
- Prefer `impl Trait` over `Box<dyn Trait>` when possible
- All public APIs should have doc comments
- Integration tests in `tests/`, unit tests in modules

## Testing

- **Unit tests**: Test routing logic and components in isolation with mocks
- **Integration tests**: Spin up test server with `--mock` flag, make real HTTP calls
- All tests must pass before submitting a PR: `cargo test`

## Submitting a Pull Request

1. Fork the repository
2. Create a feature branch from `main`
3. Make your changes with clear, focused commits
4. Ensure `cargo fmt && cargo clippy -- -D warnings && cargo test` all pass
5. Open a PR with a description of what and why

## Architecture Overview

See [DEVELOPMENT.md](./DEVELOPMENT.md) for detailed architecture documentation including request flow diagrams, component descriptions, database schema, and the full project file map.
