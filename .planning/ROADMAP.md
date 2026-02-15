# Roadmap: arbstr

## Milestones

- SHIPPED **v1 Reliability and Observability** -- Phases 1-4 (shipped 2026-02-04)
- IN PROGRESS **v1.1 Secrets Hardening** -- Phases 5-7

## Phases

<details>
<summary>SHIPPED v1 Reliability and Observability (Phases 1-4) -- SHIPPED 2026-02-04</summary>

- [x] Phase 1: Foundation (2/2 plans) -- completed 2026-02-02
- [x] Phase 2: Request Logging (4/4 plans) -- completed 2026-02-04
- [x] Phase 3: Response Metadata (1/1 plan) -- completed 2026-02-04
- [x] Phase 4: Retry and Fallback (3/3 plans) -- completed 2026-02-04

See: .planning/milestones/v1-ROADMAP.md for full details.

</details>

### v1.1 Secrets Hardening

**Milestone Goal:** Eliminate plaintext API keys from config files by supporting environment variable injection and convention-based lookup, and redact keys from all output surfaces.

- [ ] **Phase 5: Secret Type Foundation** - Migrate api_key to SecretString with automatic Debug redaction and zeroize-on-drop
- [ ] **Phase 6: Environment Variable Expansion** - Support ${VAR} syntax, convention-based key lookup, and key source reporting
- [ ] **Phase 7: Output Surface Hardening** - File permission warnings, masked key display, and plaintext literal warnings

## Phase Details

### Phase 5: Secret Type Foundation
**Goal**: API keys are protected by the Rust type system -- Debug, Display, and tracing never expose key values
**Depends on**: Phase 4 (v1 complete)
**Requirements**: SEC-01, SEC-02, RED-02
**Success Criteria** (what must be TRUE):
  1. Running `cargo run -- providers -c config.toml` with a real API key shows `[REDACTED]` instead of the key value
  2. The `/providers` JSON endpoint returns redacted key information, never plaintext keys
  3. Debug-logging the config (RUST_LOG=arbstr=debug) produces output with `[REDACTED]` where keys would appear
  4. All existing tests pass with the new SecretString type (no regressions)
**Plans:** 1 plan

Plans:
- [ ] 05-01-PLAN.md -- Define ApiKey newtype, propagate through all layers, add redaction tests

### Phase 6: Environment Variable Expansion
**Goal**: Users can keep API keys out of config files entirely, using environment variables with explicit references or convention-based auto-discovery
**Depends on**: Phase 5
**Requirements**: ENV-01, ENV-02, ENV-03, ENV-04, ENV-05
**Success Criteria** (what must be TRUE):
  1. Setting `api_key = "${MY_KEY}"` in config and exporting `MY_KEY=cashuA...` starts arbstr with that key resolved
  2. Referencing `${MISSING_VAR}` in config causes startup to fail with a clear error naming the variable and provider
  3. Omitting `api_key` for a provider named "alpha" and exporting `ARBSTR_ALPHA_API_KEY=cashuA...` results in arbstr using that key
  4. Startup logs show per-provider key source (e.g., "provider alpha: key from env-expanded" or "provider beta: key from convention") without revealing key values
  5. Running `cargo run -- check -c config.toml` reports which env var references resolve and which providers have keys available
**Plans**: TBD

Plans:
- [ ] 06-01: TBD
- [ ] 06-02: TBD

### Phase 7: Output Surface Hardening
**Goal**: All remaining output surfaces are audited and hardened -- users get actionable warnings about config hygiene and can verify key identity without seeing full keys
**Depends on**: Phase 6
**Requirements**: RED-01, RED-03, RED-04
**Success Criteria** (what must be TRUE):
  1. When config.toml has permissions more open than 0600, startup emits a warning naming the file and its actual permissions
  2. The `/providers` endpoint and `providers` CLI show a masked key prefix (e.g., `cashuA...***`) so users can verify which key is loaded without seeing it
  3. When a provider has a literal plaintext `api_key = "cashuA..."` in config (no `${}` expansion), startup emits a warning recommending environment variable usage
**Plans**: TBD

Plans:
- [ ] 07-01: TBD

## Progress

**Execution Order:**
Phases execute in numeric order: 5 -> 6 -> 7

| Phase | Milestone | Plans Complete | Status | Completed |
|-------|-----------|----------------|--------|-----------|
| 1. Foundation | v1 | 2/2 | Complete | 2026-02-02 |
| 2. Request Logging | v1 | 4/4 | Complete | 2026-02-04 |
| 3. Response Metadata | v1 | 1/1 | Complete | 2026-02-04 |
| 4. Retry and Fallback | v1 | 3/3 | Complete | 2026-02-04 |
| 5. Secret Type Foundation | v1.1 | 0/1 | Not started | - |
| 6. Environment Variable Expansion | v1.1 | 0/? | Not started | - |
| 7. Output Surface Hardening | v1.1 | 0/? | Not started | - |
