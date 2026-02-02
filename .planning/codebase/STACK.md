# Technology Stack

**Analysis Date:** 2026-02-02

## Languages

**Primary:**
- Rust 2021 edition - All application code, CLI, proxy server, and core logic

## Runtime

**Environment:**
- Tokio async runtime 1.x - Asynchronous task execution for HTTP server and provider communication

**Package Manager:**
- Cargo - Rust package management

## Frameworks

**Core:**
- axum 0.7 - Web framework for HTTP server with macros support, used for `/v1/chat/completions`, `/v1/models`, `/health`, `/providers` endpoints
- tower 0.4 - HTTP middleware and utilities
- tower-http 0.5 - HTTP layer middleware (CORS, request tracing)

**HTTP Client:**
- reqwest 0.12 - HTTP client with JSON and streaming support for provider communication

**CLI:**
- clap 4.x - Command-line argument parsing with derive macros

**Configuration:**
- toml 0.8 - TOML parsing for configuration files
- config 0.14 - Configuration management

**Logging:**
- tracing 0.1 - Structured logging framework
- tracing-subscriber 0.3 - Tracing subscriber with environment filter support

## Key Dependencies

**Critical:**
- tokio 1.x with "full" features - Async runtime with all utilities (TcpListener, task spawning, etc.)
- serde 1.x with derive feature - Serialization/deserialization for JSON and TOML
- serde_json 1.x - JSON handling for request/response serialization
- thiserror 1.x - Error type derivation for OpenAI-compatible error responses
- sqlx 0.8 - SQLite database client with tokio runtime support (prepared for data logging)

**Infrastructure:**
- uuid 1.x with v4 feature - Unique identifier generation (available but not yet used)
- chrono 0.4 - DateTime handling with serde support (available but not yet used)
- futures 0.3 - Stream utilities for request/response streaming
- anyhow 1.x - Error handling for CLI operations

**Development:**
- tokio-test 0.4 - Testing utilities for async code
- wiremock 0.6 - HTTP mocking for integration tests
- tempfile 3.x - Temporary file/directory creation for testing

## Configuration

**Environment:**
- Configuration via TOML file (default: `config.toml`)
- Environment variables supported:
  - `RUST_LOG` - Tracing log level (e.g., `arbstr=debug,tower_http=trace`)
  - `ARBSTR_CONFIG` - Path to config file (default: `./config.toml`)
  - `DATABASE_URL` - SQLite database path (default: `./arbstr.db`)

**Build:**
- `Cargo.toml` - Package manifest with dependencies and binary definition
- Binary name: `arbstr` at `src/main.rs`

## Configuration Structure

**Server Config** (`config.toml`):
- `[server]` section with `listen` address (default: `127.0.0.1:8080`)
- `[database]` section with SQLite path (default: `./arbstr.db`, supports in-memory `:memory:`)

**Provider Config** (`[[providers]]`):
- Multiple providers via TOML array-of-tables
- Required fields: `name`, `url`, `models` (list), `input_rate`, `output_rate`
- Optional fields: `api_key` (Bearer token for authorization), `base_fee`
- Rates specified in satoshis per 1000 tokens

**Policies Config** (`[policies]`):
- `default_strategy`: "cheapest", "lowest_latency", "round_robin"
- `[[policies.rules]]` for policy rules with:
  - `name`, `allowed_models`, `strategy`
  - Optional: `max_sats_per_1k_output`, `keywords` for heuristic matching

**Logging Config** (`[logging]`):
- `level`: Log level (trace, debug, info, warn, error)
- `log_requests`: Boolean for request logging to database

## Platform Requirements

**Development:**
- Rust toolchain (2021 edition)
- Cargo
- SQLite (for database operations)

**Production:**
- Linux/macOS/Windows with Rust runtime support
- TCP port for proxy server (configurable, default 8080)
- Local filesystem for SQLite database (or in-memory option)
- Outbound HTTPS access to configured LLM providers

---

*Stack analysis: 2026-02-02*
