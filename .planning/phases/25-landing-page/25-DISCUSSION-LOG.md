# Phase 25: Landing Page - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-10
**Phase:** 25-landing-page
**Areas discussed:** Tech stack and hosting, Page structure and content, Visual design direction, Getting started flow

---

## Tech Stack

| Option | Description | Selected |
|--------|-------------|----------|
| Plain HTML + CSS | Single index.html, no build step, no dependencies | ✓ |
| Astro static site | Component-based SSG, zero JS default | |
| HTML + Tailwind CDN | Utility-first CSS via CDN | |

**User's choice:** Plain HTML + CSS
**Notes:** Zero-dependency, deploys anywhere.

---

## Hosting

| Option | Description | Selected |
|--------|-------------|----------|
| GitHub Pages from /docs | Free, HTTPS, custom domain. In arbstr repo. | ✓ |
| Separate arbstr-www repo | Clean separation but adds overhead | |
| Cloudflare Pages | Global CDN, more features | |

**User's choice:** GitHub Pages from /docs in arbstr repo
**Notes:** No separate repo needed.

---

## Page Sections

| Option | Description | Selected |
|--------|-------------|----------|
| Hero with tagline | Name, value prop, CTA | ✓ |
| How it works | 3-4 step visual flow | ✓ |
| Anti-token manifesto | Bitcoin-native differentiator | ✓ |
| Getting started | Terminal-style code blocks | ✓ |

**User's choice:** All four sections
**Notes:** Single page with all sections.

---

## Visual Design

| Option | Description | Selected |
|--------|-------------|----------|
| Dark theme, developer-focused | Dark bg, Bitcoin orange, monospace code | ✓ |
| Light theme, clean minimal | White bg, professional | |
| You decide | Claude's discretion | |

**User's choice:** Dark theme, developer-focused
**Notes:** #0a0a0a bg, #f7931a Bitcoin orange accent, Inter + JetBrains Mono fonts.

---

## Getting Started Flow

| Option | Description | Selected |
|--------|-------------|----------|
| Quick start snippet | 3 steps: clone, configure, compose up | ✓ |
| Full walkthrough | Step-by-step with screenshots | |
| Just a CTA button | Link to README | |

**User's choice:** Quick start snippet
**Notes:** Terminal-style, copy-paste ready. Links to README for full docs.

---

## Claude's Discretion

- Exact copy/wording, responsive design, logo treatment, CSS animations, meta tags

## Deferred Ideas

None — discussion stayed within phase scope
