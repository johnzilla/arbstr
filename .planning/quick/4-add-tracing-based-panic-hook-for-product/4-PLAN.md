---
phase: quick-4
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - src/main.rs
autonomous: true
requirements:
  - QUICK-4
must_haves:
  truths:
    - "Panics are logged via tracing::error! with location, message, and backtrace before process terminates"
    - "The hook is installed after tracing subscriber init but before any application logic"
    - "Existing tests continue to pass (no interference with catch_unwind in stream.rs)"
  artifacts:
    - path: "src/main.rs"
      provides: "Custom panic hook using std::panic::set_hook + tracing::error!"
      contains: "set_hook"
  key_links:
    - from: "std::panic::set_hook"
      to: "tracing::error!"
      via: "Panic hook closure captures panic info and emits structured tracing event"
      pattern: "set_hook.*tracing::error"
---

<objective>
Install a custom panic hook that logs panic details (message, location, backtrace) via `tracing::error!` so panics are captured by whatever tracing subscriber is active (structured JSON logs, log aggregators, etc.) rather than only appearing on raw stderr.

Purpose: Production panic observability through the existing tracing infrastructure.
Output: Modified `src/main.rs` with panic hook installed immediately after tracing subscriber init.
</objective>

<execution_context>
@/home/john/.claude/get-shit-done/workflows/execute-plan.md
@/home/john/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/main.rs
</context>

<interfaces>
<!-- From src/main.rs (lines 56-62): tracing subscriber init block -->
```rust
tracing_subscriber::registry()
    .with(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "arbstr=info,tower_http=info".into()),
    )
    .with(tracing_subscriber::fmt::layer())
    .init();
```
<!-- The panic hook must be installed AFTER .init() and BEFORE Cli::parse() on line 64 -->
</interfaces>

<tasks>

<task type="auto">
  <name>Task 1: Add tracing-based panic hook to main.rs</name>
  <files>src/main.rs</files>
  <action>
Add a custom panic hook immediately after the tracing subscriber `.init()` call (after line 62, before `let cli = Cli::parse()`).

Implementation:

```rust
// Install panic hook that logs via tracing instead of raw stderr
std::panic::set_hook(Box::new(|info| {
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    };

    let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));

    tracing::error!(
        panic.message = %payload,
        panic.location = location.as_deref().unwrap_or("unknown"),
        "panic occurred"
    );
}));
```

Key points:
- No additional crates needed -- uses only `std::panic::set_hook` and `tracing::error!`
- Extract panic message from both `&str` and `String` payloads (covers the two standard panic payload types)
- Extract file:line:column from `PanicInfo::location()`
- Use structured tracing fields (`panic.message`, `panic.location`) so log aggregators can filter/alert on these
- Do NOT include `std::backtrace::Backtrace` capture in the hook itself -- backtraces are controlled by the `RUST_BACKTRACE` env var and the default hook already prints to stderr before the process aborts. The tracing hook adds structured log capture, not backtrace replacement.
- The existing `catch_unwind` in `stream.rs` is unaffected -- `set_hook` fires for ALL panics (caught or not), which is correct: we want visibility even for caught panics in SSE processing.
  </action>
  <verify>
    <automated>cd /home/john/vault/projects/github.com/arbstr && cargo test 2>&1 && cargo clippy -- -D warnings 2>&1</automated>
  </verify>
  <done>
- `std::panic::set_hook` call exists in main.rs between tracing init and CLI parse
- Panic hook logs message and location via `tracing::error!`
- All existing tests pass
- No clippy warnings
  </done>
</task>

</tasks>

<verification>
1. `cargo test` -- all existing tests pass (no regression from panic hook)
2. `cargo clippy -- -D warnings` -- no warnings
3. Manual spot-check: `grep -n 'set_hook' src/main.rs` shows the hook installed after tracing init
</verification>

<success_criteria>
- Panic hook installed in main.rs after tracing subscriber init
- Panics emit structured tracing::error! events with message and location
- All existing tests pass, no clippy warnings
</success_criteria>

<output>
After completion, create `.planning/quick/4-add-tracing-based-panic-hook-for-product/4-SUMMARY.md`
</output>
