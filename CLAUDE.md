# CLAUDE.md - Development Guide for arbstr

The full development guide (architecture, config, schema, key files, CI) lives in DEVELOPMENT.md and is imported here:

@DEVELOPMENT.md

## Notes for Claude

- This is an early-stage project, prioritize working code over perfection
- Providers are config, not code: any OpenAI-compatible endpoint is a `[[providers]]` entry
- Cost calculations use satoshis (sats) as the unit
- OpenAI API compatibility is critical - test against real clients
- The policy engine should be easily extensible for future ML-based classification
