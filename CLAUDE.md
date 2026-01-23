# CLAUDE.md - Development Guide for arbstr

## Project Overview

arbstr is an intelligent LLM routing and cost optimization layer for the Routstr decentralized marketplace. It acts as a local proxy between your applications and Routstr providers, selecting the optimal provider based on cost, policies, and constraints.

## Quick Reference

```bash
# Build
cargo build --release

# Run tests
cargo test

# Run with debug logging
RUST_LOG=arbstr=debug cargo run

# Run the proxy server
cargo run -- serve --config config.toml

# Format code
cargo fmt

# Lint
cargo clippy -- -D warnings
```

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌──────────────────┐
│ Your App    │────▶│   arbstr    │────▶│ Routstr Provider │
│ (OpenAI API)│     │   (proxy)   │     │    (cheapest)    │
└─────────────┘     └─────────────┘     └──────────────────┘
                           │
                    ┌──────┴──────┐
                    │ Policy      │
                    │ Engine      │
                    │ + SQLite    │
                    └─────────────┘
```

### Key Components

- **Proxy Server** (`src/proxy/`): OpenAI-compatible HTTP server using axum
- **Router** (`src/router/`): Provider selection logic, cost optimization
- **Providers** (`src/providers/`): Backend adapters for Routstr providers
- **Policy Engine** (`src/policy/`): Constraint matching and heuristics
- **Storage** (`src/storage/`): SQLite for logging, costs, learned patterns

## Tech Stack

- **Runtime**: Tokio async
- **HTTP Server**: axum
- **HTTP Client**: reqwest
- **Database**: SQLite via sqlx
- **Serialization**: serde + serde_json
- **CLI**: clap
- **Config**: toml + config crate
- **Logging**: tracing + tracing-subscriber

## Code Conventions

- Use `thiserror` for error types
- Async everywhere (no blocking in async context)
- Prefer `impl Trait` over `Box<dyn Trait>` when possible
- All public APIs should have doc comments
- Integration tests in `tests/`, unit tests in modules

## Configuration

Config file: `config.toml`

```toml
[server]
listen = "127.0.0.1:8080"

[providers]
# Providers loaded from routstr or manually configured
endpoints = [
    { name = "provider-a", url = "https://...", priority = 1 },
]

[policies]
default = "cheapest"

[[policies.rules]]
name = "code_generation"
allowed_models = ["claude-3.5-sonnet", "gpt-4o"]
strategy = "lowest_cost"
max_sats_per_1k_output = 50
```

## Database Schema (SQLite)

```sql
-- Request log for cost tracking and learning
CREATE TABLE requests (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    policy TEXT,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cost_sats INTEGER,
    latency_ms INTEGER,
    success BOOLEAN
);

-- Learned input/output ratios per policy
CREATE TABLE token_ratios (
    policy TEXT PRIMARY KEY,
    avg_ratio REAL,
    sample_count INTEGER
);
```

## Testing Strategy

- **Unit tests**: Mock providers, test routing logic in isolation
- **Integration tests**: Spin up test server, make real HTTP calls
- **Mock mode**: `--mock` flag to use fake providers with configurable delays/costs
- **Future**: Bitcoin testnet/signet for payment testing

## MVP Milestones

1. **M1**: Basic proxy pass-through to single hardcoded provider
2. **M2**: Multiple providers, cheapest selection based on advertised rates
3. **M3**: Policy constraints (allowed models, max cost per request)
4. **M4**: Request logging and cost tracking dashboard
5. **M5**: Heuristic-based automatic policy classification

## Key Files

```
src/
├── main.rs           # CLI entry point
├── lib.rs            # Library root, re-exports
├── config.rs         # Configuration structs
├── proxy/
│   ├── mod.rs
│   ├── server.rs     # axum server setup
│   └── handlers.rs   # /v1/chat/completions, etc.
├── router/
│   ├── mod.rs
│   ├── selector.rs   # Provider selection algorithm
│   └── cost.rs       # Cost calculation
├── providers/
│   ├── mod.rs
│   ├── provider.rs   # Provider trait
│   └── routstr.rs    # Routstr-specific adapter
├── policy/
│   ├── mod.rs
│   ├── engine.rs     # Policy matching
│   └── heuristics.rs # Keyword-based classification
└── storage/
    ├── mod.rs
    └── sqlite.rs     # Database operations
```

## Environment Variables

- `RUST_LOG`: Log level (e.g., `arbstr=debug,tower_http=trace`)
- `ARBSTR_CONFIG`: Path to config file (default: `./config.toml`)
- `DATABASE_URL`: SQLite path (default: `./arbstr.db`)

## Notes for Claude

- This is an early-stage project, prioritize working code over perfection
- When adding providers, implement the `Provider` trait
- Cost calculations use satoshis (sats) as the unit
- OpenAI API compatibility is critical - test against real clients
- The policy engine should be easily extensible for future ML-based classification
