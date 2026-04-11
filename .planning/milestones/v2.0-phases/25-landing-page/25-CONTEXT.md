# Phase 25: Landing Page - Context

**Gathered:** 2026-04-10
**Status:** Ready for planning

<domain>
## Phase Boundary

Create arbstr.com as a single-page landing site that communicates the marketplace vision (NiceHash for AI inference), explains the Bitcoin-native settlement model, and onboards developers with a quick start guide. Static HTML + CSS deployed via GitHub Pages.

</domain>

<decisions>
## Implementation Decisions

### Tech Stack
- **D-01:** Plain HTML + CSS. Single `index.html` with linked `style.css`. No JavaScript framework, no build step, no dependencies.
- **D-02:** No Tailwind, no React, no Astro. Pure HTML/CSS keeps it zero-dependency and fast.

### Hosting and Deployment
- **D-03:** GitHub Pages from `/docs` folder in the arbstr repo. Source: `/docs` branch/folder configured in GitHub Settings > Pages.
- **D-04:** CNAME file in `/docs` for arbstr.com custom domain. GitHub Pages handles HTTPS automatically.
- **D-05:** No separate repo — landing page lives alongside the Rust codebase in `/docs`.

### Page Structure
- **D-06:** Single page with 4 sections in order:
  1. **Hero** — Project name, one-line value prop ("NiceHash for AI inference"), CTA buttons (Get Started / GitHub)
  2. **How it works** — 3-4 step visual: your app → arbstr → cheapest provider → Bitcoin settlement
  3. **Anti-token manifesto** — Short, punchy: "No tokens. No staking. No governance theater. Just sats."
  4. **Getting started** — Terminal-style code block with 3 steps (clone, configure, compose up)
- **D-07:** Footer with GitHub link, license info, and brief project description.

### Visual Design
- **D-08:** Dark theme, developer-focused. Color palette:
  - Background: `#0a0a0a` (near-black)
  - Text: `#e5e5e5` (light gray)
  - Accent: `#f7931a` (Bitcoin orange)
  - Code blocks: `#1a1a2e` (dark navy)
- **D-09:** Typography:
  - Body: Inter or system sans-serif stack
  - Code: JetBrains Mono or system monospace
- **D-10:** Feel: Technical, confident, no marketing fluff. Terminal-style code blocks with syntax highlighting via CSS only.

### Getting Started Content
- **D-11:** Quick start snippet — 3 steps:
  1. `git clone https://github.com/johnzilla/arbstr-node && cd arbstr-node`
  2. `cp .env.example .env` (edit with Lightning node details)
  3. `docker compose up`
  Followed by a curl example hitting localhost:8080.
- **D-12:** Links to GitHub README for full documentation. Landing page is not a docs site.

### Claude's Discretion
- Exact copy/wording for each section (hero tagline, manifesto text, feature descriptions)
- Responsive design breakpoints and mobile layout
- Whether to include a simple logo/wordmark or just text
- Whether to add subtle CSS animations (fade-in on scroll, etc.)
- Exact "how it works" visual treatment (ASCII art, CSS diagrams, or icon-based steps)
- Meta tags, Open Graph, and SEO basics

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Project Brand
- `.planning/PROJECT.md` — Core value ("NiceHash for AI inference"), anti-token positioning, product vision
- `.planning/REQUIREMENTS.md` — DEPLOY-04 requirement for landing page

### Getting Started Source Material
- `/home/john/vault/projects/github.com/arbstr-node/docker-compose.yml` — Actual compose file referenced in getting started
- `/home/john/vault/projects/github.com/arbstr-node/.env.example` — Environment variables users need to configure
- `/home/john/vault/projects/github.com/arbstr-node/config.toml` — Config file users customize

### Architecture Reference
- `.planning/research/ARCHITECTURE.md` — System architecture diagram (can inform "how it works" visual)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- No existing frontend code — this is a greenfield page
- `/docs` directory does not exist yet — needs to be created

### Established Patterns
- None — first frontend deliverable in this project

### Integration Points
- GitHub Pages configured via Settings > Pages > Source: `/docs`
- CNAME file in `/docs` for custom domain
- Links to `github.com/johnzilla/arbstr-node` for getting started
- Links to `github.com/johnzilla/arbstr` for source code

</code_context>

<specifics>
## Specific Ideas

- Bitcoin orange (`#f7931a`) as the primary accent color — ties to Bitcoin-native identity
- "No tokens. No staking. No governance theater. Just sats." — exact manifesto line from PROJECT.md
- Terminal-style getting started section that looks like a real terminal
- "NiceHash for AI inference" as the hero tagline

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 25-landing-page*
*Context gathered: 2026-04-10*
