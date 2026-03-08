---
phase: quick-6
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - DEVELOPMENT.md
  - CONTRIBUTING.md
  - README.md
autonomous: true
requirements: [DOC-01, DOC-02, DOC-03]
must_haves:
  truths:
    - "DEVELOPMENT.md exists with full architecture, tech stack, code conventions, DB schema, testing strategy, and key file map"
    - "CONTRIBUTING.md exists with dev environment setup, test instructions, and PR submission guidelines"
    - "README.md is user-focused: what it is, how to install, how to use, links to DEVELOPMENT.md and CONTRIBUTING.md for developer details"
    - "CLAUDE.md is completely unchanged"
  artifacts:
    - path: "DEVELOPMENT.md"
      provides: "Complete developer reference (copied from CLAUDE.md with improvements)"
      contains: "## Architecture"
    - path: "CONTRIBUTING.md"
      provides: "Contributor guidelines"
      contains: "## Getting Started"
    - path: "README.md"
      provides: "User-focused project overview"
      contains: "DEVELOPMENT.md"
  key_links:
    - from: "README.md"
      to: "DEVELOPMENT.md"
      via: "markdown link"
      pattern: "\\[DEVELOPMENT\\.md\\]"
    - from: "README.md"
      to: "CONTRIBUTING.md"
      via: "markdown link"
      pattern: "\\[CONTRIBUTING\\.md\\]"
---

<objective>
Reorganize developer documentation into proper files: DEVELOPMENT.md for architecture and internals, CONTRIBUTING.md for contributor guidelines, and a slimmed-down user-focused README.md.

Purpose: Developer documentation is fragmented -- the detailed content lives in CLAUDE.md (a Claude Code instruction file that must not be modified) and overlaps with README.md. This reorganization creates proper public-facing developer docs.
Output: Three files -- DEVELOPMENT.md (new), CONTRIBUTING.md (new), README.md (updated)
</objective>

<execution_context>
@/home/john/.claude/get-shit-done/workflows/execute-plan.md
@/home/john/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@CLAUDE.md
@README.md

CRITICAL CONSTRAINT: CLAUDE.md must NOT be modified or deleted. It serves as project instructions for Claude Code and must remain as-is.
</context>

<tasks>

<task type="auto">
  <name>Task 1: Create DEVELOPMENT.md from CLAUDE.md content</name>
  <files>DEVELOPMENT.md</files>
  <action>
Create DEVELOPMENT.md by copying the content of CLAUDE.md and adapting it for a public developer audience. Specific changes from the CLAUDE.md source:

1. Change the title from "CLAUDE.md - Development Guide for arbstr" to "# Development Guide"
2. Keep ALL technical sections intact: Quick Reference, Architecture (both mermaid diagrams), Key Components, Tech Stack, Code Conventions, Configuration (full TOML example + API Key Management), Database Schema, Testing Strategy, Shipped Versions, Key Files tree, Environment Variables
3. REMOVE the "## Notes for Claude" section entirely -- that is Claude-specific instruction and not relevant to human developers
4. Add a brief intro paragraph after the title: "This document covers arbstr's architecture, internals, and development workflow. For user-facing documentation (installation, usage), see [README.md](./README.md). For contribution guidelines, see [CONTRIBUTING.md](./CONTRIBUTING.md)."
5. Keep the "## Project Overview" section as-is (it provides good context for developers too)

Do NOT modify CLAUDE.md in any way.
  </action>
  <verify>
    <automated>test -f DEVELOPMENT.md && grep -q "## Architecture" DEVELOPMENT.md && grep -q "## Database Schema" DEVELOPMENT.md && grep -q "## Key Files" DEVELOPMENT.md && ! grep -q "Notes for Claude" DEVELOPMENT.md && echo "PASS" || echo "FAIL"</automated>
  </verify>
  <done>DEVELOPMENT.md exists with all technical content from CLAUDE.md, minus the Claude-specific notes section, with cross-links to README.md and CONTRIBUTING.md</done>
</task>

<task type="auto">
  <name>Task 2: Create CONTRIBUTING.md with contributor guidelines</name>
  <files>CONTRIBUTING.md</files>
  <action>
Create CONTRIBUTING.md with these sections:

## Contributing to arbstr

Brief welcome paragraph.

### Getting Started

1. Clone the repo: `git clone https://github.com/johnzilla/arbstr.git`
2. Install Rust toolchain (stable): link to https://rustup.rs
3. Build: `cargo build`
4. Run tests: `cargo test`
5. Run with mock providers: `cargo run -- serve --mock`

### Development Workflow

- Format code before committing: `cargo fmt`
- Run clippy: `cargo clippy -- -D warnings`
- Run the full check: `cargo fmt && cargo clippy -- -D warnings && cargo test`
- Use debug logging: `RUST_LOG=arbstr=debug cargo run -- serve --mock`

### Code Style

Reference the code conventions from DEVELOPMENT.md (link to it):
- Use `thiserror` for error types
- Async everywhere (no blocking in async context)
- Prefer `impl Trait` over `Box<dyn Trait>` when possible
- All public APIs should have doc comments
- Integration tests in `tests/`, unit tests in modules

### Testing

- Unit tests: test routing logic and components in isolation with mocks
- Integration tests: spin up test server with `--mock` flag, make real HTTP calls
- All tests must pass before submitting a PR: `cargo test`

### Submitting a Pull Request

1. Fork the repository
2. Create a feature branch from `main`
3. Make your changes with clear, focused commits
4. Ensure `cargo fmt && cargo clippy -- -D warnings && cargo test` all pass
5. Open a PR with a description of what and why

### Architecture Overview

Brief pointer: "See [DEVELOPMENT.md](./DEVELOPMENT.md) for detailed architecture documentation including request flow diagrams, component descriptions, and database schema."
  </action>
  <verify>
    <automated>test -f CONTRIBUTING.md && grep -q "Getting Started" CONTRIBUTING.md && grep -q "Pull Request" CONTRIBUTING.md && grep -q "DEVELOPMENT.md" CONTRIBUTING.md && echo "PASS" || echo "FAIL"</automated>
  </verify>
  <done>CONTRIBUTING.md exists with dev setup, workflow, code style, testing, and PR guidelines, with links to DEVELOPMENT.md for deeper architecture info</done>
</task>

<task type="auto">
  <name>Task 3: Slim down README.md to be user-focused</name>
  <files>README.md</files>
  <action>
Update README.md to be user-focused by removing developer-internal content that now lives in DEVELOPMENT.md. Specific changes:

1. KEEP as-is: title, intro paragraph, mermaid diagram, "## Features", "## Quick Start", "## Configuration" (full section including API Key Management and Policy Matching), "## How Routing Works", "## CLI", "## API Endpoints", "## Roadmap", "## Related Projects", "## License"

2. REPLACE the "## Development" section (lines 194-204) with a shorter version:
```markdown
## Development

See [DEVELOPMENT.md](./DEVELOPMENT.md) for the full development guide including architecture, database schema, and internals.

```bash
cargo test                    # Run all tests
cargo run -- serve --mock     # Run with mock providers
cargo fmt && cargo clippy -- -D warnings  # Format and lint
```
```

3. REMOVE the "## Project Structure" section (lines 206-230) -- this detailed file tree now lives in DEVELOPMENT.md

4. REPLACE the "## Contributing" section with:
```markdown
## Contributing

This project is being built in public. See [CONTRIBUTING.md](./CONTRIBUTING.md) for development setup and contribution guidelines.
```

5. Ensure the old link to CLAUDE.md (line 204: "See [CLAUDE.md](./CLAUDE.md) for detailed...") is removed -- DEVELOPMENT.md replaces it as the public-facing developer doc reference.

Do NOT modify CLAUDE.md.
  </action>
  <verify>
    <automated>grep -q "DEVELOPMENT.md" README.md && grep -q "CONTRIBUTING.md" README.md && ! grep -q "CLAUDE.md" README.md && ! grep -q "├── main.rs" README.md && echo "PASS" || echo "FAIL"</automated>
  </verify>
  <done>README.md is user-focused (features, install, usage, config, API reference) with links to DEVELOPMENT.md and CONTRIBUTING.md instead of containing developer internals. No reference to CLAUDE.md. Project Structure tree removed (lives in DEVELOPMENT.md).</done>
</task>

</tasks>

<verification>
1. CLAUDE.md is byte-identical to its original (git diff CLAUDE.md shows no changes)
2. DEVELOPMENT.md contains architecture diagrams, tech stack, code conventions, DB schema, key files, env vars
3. CONTRIBUTING.md contains setup instructions, workflow, code style, testing, PR guidelines
4. README.md links to both DEVELOPMENT.md and CONTRIBUTING.md
5. README.md does NOT contain the project structure file tree or reference CLAUDE.md
6. No broken markdown links between the three files
</verification>

<success_criteria>
- Three documentation files serve distinct purposes: README (users), DEVELOPMENT (developers), CONTRIBUTING (contributors)
- CLAUDE.md is completely untouched
- README.md is shorter and user-focused, linking to the other docs for developer details
- All cross-references between docs are correct
</success_criteria>

<output>
After completion, create `.planning/quick/6-reorganize-developer-docs-development-md/6-SUMMARY.md`
</output>
