# Testing Patterns

**Analysis Date:** 2026-02-02

## Test Framework

**Runner:**
- Rust standard test harness (built into cargo)
- Dev dependencies include `tokio-test` for async test support
- No external test framework; uses `#[test]` and `#[tokio::test]` attributes

**Assertion Library:**
- Standard Rust `assert!()`, `assert_eq!()` macros
- `matches!()` macro for enum pattern matching assertions
- Example: `assert!(matches!(result, Err(Error::NoProviders { .. })))`

**Run Commands:**
```bash
cargo test              # Run all tests
cargo test --lib       # Run only library tests (unit/integration in src/)
cargo test -- --nocapture  # Show println! output
RUST_LOG=debug cargo test   # Run with logging
cargo test -- --test-threads=1  # Run sequentially if needed
```

## Test File Organization

**Location:**
- Co-located with implementation: tests defined in `#[cfg(test)]` modules within source files
- Currently tests exist in: `src/config.rs` and `src/router/selector.rs`
- No separate `tests/` directory for integration tests (not yet implemented)

**Naming:**
- Test functions use `test_` prefix: `test_parse_minimal_config`, `test_select_cheapest`
- Descriptive names indicate what is tested: `test_no_providers_for_model`, `test_policy_keyword_matching`
- Module name matches test concern: `#[cfg(test)] mod tests { ... }`

**Structure:**
```
src/config.rs
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_parse_minimal_config() { ... }
    }

src/router/selector.rs
    #[cfg(test)]
    mod tests {
        use super::*;

        fn test_providers() -> Vec<ProviderConfig> { ... }

        #[test]
        fn test_select_cheapest() { ... }
    }
```

## Test Structure

**Suite Organization:**
From `src/config.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
            [server]
            listen = "127.0.0.1:9000"
        "#;

        let config = Config::from_str(toml).unwrap();
        assert_eq!(config.server.listen, "127.0.0.1:9000");
        assert!(config.providers.is_empty());
    }
}
```

**Patterns:**
- **Setup:** Inline test data creation with raw string literals for TOML
- **Teardown:** No explicit teardown needed; data is cleaned up automatically
- **Assertion:** Direct assertions on parsed results; error cases tested with `assert!(matches!(...))`

**Test Data Factories:**
From `src/router/selector.rs`:
```rust
fn test_providers() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            name: "cheap".to_string(),
            url: "https://cheap.example.com/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
            input_rate: 5,
            output_rate: 15,
            base_fee: 0,
        },
        // ... more providers
    ]
}
```
- Helper functions create reusable test fixture objects
- Named to indicate they provide test data: `test_providers`, `test_policies`
- Used across multiple test functions

## Mocking

**Framework:** `wiremock` (included in dev-dependencies)

**Status:**
- Declared as dependency but not yet used in existing tests
- Ready for HTTP mocking when integration tests are added
- Will be used to mock upstream provider responses

**Built-in Mock Support:**
- `--mock` flag in CLI mode enables mock provider configuration
- From `src/main.rs` `mock_config()`:
  ```rust
  fn mock_config() -> Config {
      Config {
          server: ServerConfig {
              listen: "127.0.0.1:8080".to_string(),
          },
          providers: vec![
              ProviderConfig {
                  name: "mock-cheap".to_string(),
                  url: "http://localhost:9999/v1".to_string(), // Won't be called
                  // ... configured rates
              },
          ],
          // ...
      }
  }
  ```
- Mock providers configured with different cost rates to test selection logic
- Can run full proxy server without real API calls: `cargo run -- serve --mock`

**What to Mock:**
- External HTTP calls to providers (when `wiremock` is used)
- Provider responses for streaming and non-streaming paths
- Error responses to test error handling paths

**What NOT to Mock:**
- Internal routing logic (Router struct)
- Config parsing (test with real TOML strings)
- Error type construction (test actual error variants)

## Fixtures and Factories

**Test Data:**
From `src/router/selector.rs`:
```rust
fn test_providers() -> Vec<ProviderConfig> {
    vec![
        ProviderConfig {
            name: "cheap".to_string(),
            url: "https://cheap.example.com/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string(), "gpt-4o-mini".to_string()],
            input_rate: 5,
            output_rate: 15,
            base_fee: 0,
        },
        ProviderConfig {
            name: "expensive".to_string(),
            url: "https://expensive.example.com/v1".to_string(),
            api_key: None,
            models: vec!["gpt-4o".to_string(), "claude-3.5-sonnet".to_string()],
            input_rate: 10,
            output_rate: 30,
            base_fee: 1,
        },
    ]
}
```

**Location:**
- Inline within test module using helper functions
- No separate fixture files or databases
- TOML strings inline for config tests: `let toml = r#"..."#;`

## Coverage

**Requirements:** Not enforced

**View Coverage:**
```bash
# No coverage tool configured yet; standard Rust approach would be:
cargo tarpaulin --out Html  # Requires tarpaulin to be installed
# or
cargo llvm-cov              # Requires llvm-cov to be installed
```

## Test Types

**Unit Tests:**
- **Scope:** Individual functions and modules in isolation
- **Approach:** Test config parsing, router selection logic, error handling
- **Location:** `src/config.rs` and `src/router/selector.rs`
- **Examples:**
  - `test_parse_minimal_config`: Validates TOML parsing with minimal config
  - `test_select_cheapest`: Router selects cheapest provider by output rate
  - `test_no_providers_for_model`: Error handling when model unavailable
  - `test_policy_keyword_matching`: Heuristic policy matching on prompt keywords

**Integration Tests:**
- **Status:** Not yet implemented
- **Planned approach:** Would spin up actual server with `wiremock` mocking providers
- **Location:** Would be in `tests/` directory
- **Intended scope:**
  - Test full request flow: client → proxy → handler → router → provider mock
  - Verify streaming and non-streaming response paths
  - Test error propagation from providers to clients
  - OpenAI API compatibility across endpoints

**E2E Tests:**
- **Status:** Not implemented
- **Framework:** Not selected
- **Planned:** May use real Routstr testnet with actual Bitcoin/Lightning transactions

## Common Patterns

**Async Testing:**
```rust
// Would use #[tokio::test] for async tests when implemented:
#[tokio::test]
async fn test_server_startup() {
    // async code here
}
```
- Currently no async tests in codebase
- dev-dependency `tokio-test` available for utilities
- Will be needed for handler and server tests

**Error Testing:**
```rust
#[test]
fn test_no_providers_for_model() {
    let router = Router::new(test_providers(), vec![], "cheapest".to_string());
    let result = router.select("nonexistent-model", None, None);
    assert!(matches!(result, Err(Error::NoProviders { .. })));
}
```
- Pattern: call method that should error, match on `Result`
- Use `matches!()` macro for enum pattern matching
- Verify specific error variant, not just `is_err()`

**Configuration Testing:**
```rust
#[test]
fn test_parse_full_config() {
    let toml = r#"
        [server]
        listen = "0.0.0.0:8080"
        [[providers]]
        name = "test-provider"
        url = "https://example.com/v1"
        models = ["gpt-4o", "claude-3.5-sonnet"]
        input_rate = 10
        output_rate = 30
        base_fee = 1
    "#;

    let config = Config::from_str(toml).unwrap();
    assert_eq!(config.providers.len(), 1);
    assert_eq!(config.providers[0].name, "test-provider");
}
```
- Pattern: inline TOML strings with `r#"..."#` raw string literal
- Validate parsed values match input
- Test multiple provider configurations and policy rules

## Testing Strategy

**Current Coverage:**
- Config parsing: 2 tests (minimal and full configurations)
- Router selection: 3 tests (cheapest selection, no providers, policy keyword matching)
- Total: 5 unit tests

**Gap Areas (High Priority):**
- HTTP handlers and request forwarding
- Streaming vs non-streaming response paths
- Provider error handling and propagation
- OpenAI API response format compliance
- Authentication header handling

**Next Steps for Test Expansion:**
1. Add `wiremock` integration tests in `tests/` directory
2. Test `chat_completions` handler with mocked upstream provider
3. Test streaming response streaming path (`Body::from_stream`)
4. Test error responses match OpenAI format
5. Add async test infrastructure with `#[tokio::test]`

---

*Testing analysis: 2026-02-02*
