# Architecture

**Analysis Date:** 2026-02-02

## Pattern Overview

**Overall:** Layered proxy pattern with policy-driven routing

**Key Characteristics:**
- OpenAI-compatible HTTP API layer passes requests through a provider selection router
- Stateless request handling with async/await throughout
- Configuration-driven provider and policy management
- Cost-optimized provider selection based on advertised rates and constraints

## Layers

**HTTP Server Layer:**
- Purpose: Accept OpenAI-compatible chat completion requests and serve provider metadata
- Location: `src/proxy/`
- Contains: Request/response type definitions, HTTP handlers, axum server setup
- Depends on: Router layer for provider selection, Config for server settings
- Used by: External OpenAI clients making HTTP requests

**Router Layer:**
- Purpose: Select the optimal provider based on model support, cost, and policy constraints
- Location: `src/router/`
- Contains: Provider selection logic, policy matching, cost calculation
- Depends on: Config for provider and policy definitions
- Used by: HTTP handlers to select which provider to forward requests to

**Configuration Layer:**
- Purpose: Parse and validate TOML configuration files, define all configurable aspects
- Location: `src/config.rs`
- Contains: Server config, provider definitions, policy rules, logging settings
- Depends on: serde for deserialization, toml for parsing
- Used by: Server initialization and router setup

**Error Handling Layer:**
- Purpose: Provide OpenAI-compatible error responses for all failure modes
- Location: `src/error.rs`
- Contains: Error enum with OpenAI-compatible JSON responses
- Depends on: axum for response conversion
- Used by: All other layers to return errors

## Data Flow

**Request Processing Flow:**

1. Client sends POST to `/v1/chat/completions` with OpenAI-compatible JSON
2. `chat_completions` handler extracts policy from `X-Arbstr-Policy` header and request body
3. Handler calls `Router::select()` with model, optional policy name, and user prompt
4. Router finds matching policy by:
   - First trying explicit policy header match (`X-Arbstr-Policy: {name}`)
   - Falling back to keyword heuristics on user prompt if no header match
   - Returning None if no policy found
5. Router filters provider candidates by:
   - Model support (empty model list = supports all)
   - Policy constraints (allowed models, max cost per 1k output tokens)
6. Router selects provider using strategy (currently "cheapest" = lowest output rate)
7. Handler forwards request to selected provider via `reqwest` HTTP client
8. Response streamed or buffered back to client based on `stream` flag
9. Non-streaming responses include `arbstr_provider` metadata field

**State Management:**
- `AppState` in `src/proxy/server.rs` holds shared references via `Arc`:
  - `router`: Immutable `ProviderRouter` with all providers and policies
  - `http_client`: reqwest `Client` for forwarding requests
  - `config`: Immutable configuration
- Configuration loaded once at startup from TOML or mock
- No mutable state shared across requests

## Key Abstractions

**Router:**
- Purpose: Encapsulates provider selection logic with policy awareness
- Examples: `src/router/selector.rs` lines 30-183
- Pattern: Functional filtering and selection using iterators; strategies pluggable via string match

**ChatCompletionRequest:**
- Purpose: OpenAI-compatible request deserialization with arbstr extensions
- Examples: `src/proxy/types.rs` lines 6-26
- Pattern: serde Deserialize with optional fields; includes helper method `user_prompt()`

**AppState:**
- Purpose: Shared application context passed through axum handlers
- Examples: `src/proxy/server.rs` lines 16-22
- Pattern: Clone-able wrapper over Arc<ProviderRouter>, Arc<Config>, and Client

**Policy Rule:**
- Purpose: Match requests and constrain provider selection
- Examples: `src/config.rs` lines 89-105
- Pattern: Keyword matching for heuristic classification; allowed models and max cost constraints

## Entry Points

**CLI Entry Point:**
- Location: `src/main.rs`
- Triggers: Command line invocation with subcommand
- Responsibilities: Parse CLI args, load config, initialize logging, dispatch to serve/check/providers commands

**HTTP Entry Point:**
- Location: `src/proxy/handlers.rs` lines 19-129 (`chat_completions`)
- Triggers: POST /v1/chat/completions
- Responsibilities: Validate request, select provider, forward upstream, stream/return response

**Server Initialization:**
- Location: `src/proxy/server.rs` lines 38-68 (`run_server`)
- Triggers: `serve` CLI command
- Responsibilities: Load config, create provider router, bind TCP listener, start axum server

## Error Handling

**Strategy:** OpenAI-compatible JSON error responses with appropriate HTTP status codes

**Patterns:**
- `NoProviders`: Model unsupported across all configured providers → HTTP 400 Bad Request
- `NoPolicyMatch`: Policy constraints eliminated all candidate providers → HTTP 400 Bad Request
- `Provider`: Upstream provider unreachable or returned error → HTTP 502 Bad Gateway
- `BadRequest`: Invalid request structure → HTTP 400 Bad Request
- `Config`: Configuration error → HTTP 500 Internal Server Error
- All errors return JSON with `error.message`, `error.type`, `error.code` fields (lines 47-52 in `src/error.rs`)

## Cross-Cutting Concerns

**Logging:**
- Framework: `tracing` and `tracing-subscriber`
- Pattern: Structured logging with span contexts; middleware added via `TraceLayer` from `tower_http`
- Key events logged: Request received, policy matched, provider selected, provider errors
- Configuration: `RUST_LOG` environment variable controls level (default: `arbstr=info,tower_http=info`)

**Validation:**
- Configuration validated on parse (lines 154-169 in `src/config.rs`): provider URLs must be non-empty
- Request structure validated by serde deserialization (missing fields fail at JSON parse)
- Model availability checked against provider lists (lines 68-78 in `src/router/selector.rs`)

**Authentication:**
- Provider API keys stored in `ProviderConfig` (optional field)
- Keys passed as `Authorization: Bearer {api_key}` header in upstream requests (lines 63-65 in `src/proxy/handlers.rs`)
- No client authentication to arbstr proxy itself currently implemented

---

*Architecture analysis: 2026-02-02*
