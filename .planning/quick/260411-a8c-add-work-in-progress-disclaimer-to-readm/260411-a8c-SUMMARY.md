# Quick Task 260411-a8c: Add WIP disclaimer — Summary

**Status:** Complete
**Date:** 2026-04-11
**Commit:** d107fca

## What Changed

1. **README.md** — Added understated blockquote after the tagline:
   > **Early-stage software** — under active development. APIs and configuration may change.

2. **docs/index.html** — Added `<p class="wip-notice">` in the hero section between sub-tagline and CTA buttons.

3. **docs/style.css** — Added `.wip-notice` rule (muted gray, 0.85rem, subtle top margin).

## Verification

- README contains "active development" notice: PASS
- Landing page has wip-notice element: PASS
- CSS has .wip-notice styling: PASS
- No alarming language (WARNING, DANGER, CAUTION): PASS
