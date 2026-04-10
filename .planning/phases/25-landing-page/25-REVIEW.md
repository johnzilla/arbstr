---
phase: 25-landing-page
reviewed: 2026-04-10T00:00:00Z
depth: standard
files_reviewed: 2
files_reviewed_list:
  - docs/index.html
  - docs/style.css
findings:
  critical: 0
  warning: 3
  info: 4
  total: 7
status: issues_found
---

# Phase 25: Code Review Report

**Reviewed:** 2026-04-10
**Depth:** standard
**Files Reviewed:** 2
**Status:** issues_found

## Summary

Reviewed the landing page HTML and CSS. The markup is clean, well-structured, and responsive. No security vulnerabilities or hardcoded secrets were found. Three issues require attention: a copy-paste breakage bug in the terminal code block (HTML tags embedded in `<pre>` will be copied as raw markup), missing Subresource Integrity on Google Fonts external resources, and a brittle `opacity` pattern on button hover states. Four informational items cover accessibility and SEO gaps.

---

## Warnings

### WR-01: HTML tags embedded in `<pre><code>` break copy-paste

**File:** `docs/index.html:80`
**Issue:** A `<span class="comment">` tag is placed inside the `<pre><code>` block to style the inline comment. When a user copies the terminal block, they copy the raw HTML — `<span class="comment"># edit with your secrets</span>` — rather than `# edit with your secrets`. The page has no JavaScript to intercept copy events, so this is a real functional defect. Anyone following the getting-started instructions will paste broken shell input if they select across that line.
**Fix:** Remove the `<span>` and instead use a CSS `:before`/`::after` trick (hard to target inline) or, more practically, split the line into two `<code>` segments with surrounding text, or just remove the comment coloring entirely and leave the comment as plain text:

```html
<pre><code>git clone https://github.com/johnzilla/arbstr-node &amp;&amp; cd arbstr-node
cp .env.example .env  # edit with your secrets
docker compose up</code></pre>
```

The CSS class `.terminal .comment` can be removed from `style.css` as well if no longer needed.

---

### WR-02: No Subresource Integrity (SRI) on Google Fonts stylesheet

**File:** `docs/index.html:14`
**Issue:** The Google Fonts stylesheet is loaded without an `integrity` attribute. If the Google CDN were compromised or served a malicious stylesheet, it could inject arbitrary CSS (CSS-based data exfiltration, UI redressing). SRI is the standard mitigation for third-party stylesheets.
**Fix:** Generate an SRI hash for the specific font URL and add `integrity` and `crossorigin` attributes:

```html
<link
  href="https://fonts.googleapis.com/css2?family=Inter:wght@400;600&family=JetBrains+Mono&display=swap"
  rel="stylesheet"
  crossorigin="anonymous"
  integrity="sha384-<hash-here>">
```

Note: Google Fonts responses vary by user-agent, which makes stable SRI hashes difficult. A practical alternative is to self-host the fonts (download via `google-webfonts-helper`) and serve them from the same origin, eliminating the third-party dependency entirely.

---

### WR-03: Brittle opacity override on button hover

**File:** `docs/style.css:128-130, 139-141`
**Issue:** The global `a:hover { opacity: 0.85 }` rule (line 40-42) applies to all anchor elements including `.btn` anchors. Both `.btn-primary:hover` and `.btn-outline:hover` explicitly reset `opacity: 1` to counteract this. Any future button variant that omits `opacity: 1` will unexpectedly dim on hover. The root cause is applying a global opacity rule to `a` rather than scoping it to content links.
**Fix:** Scope the default hover opacity to content links only, rather than all anchors:

```css
/* Instead of: */
a:hover {
  opacity: 0.85;
}

/* Use: */
a:not(.btn):hover {
  opacity: 0.85;
}
```

Then remove the `opacity: 1` overrides from `.btn-primary:hover` and `.btn-outline:hover`.

---

## Info

### IN-01: Decorative terminal-dot spans not hidden from screen readers

**File:** `docs/index.html:75-78, 83-87`
**Issue:** The three colored dot spans inside `.terminal-bar` are purely decorative (macOS window chrome aesthetic). Screen readers will encounter and attempt to announce three empty, unlabeled `<span>` elements per terminal block (six total). Adding `aria-hidden` removes them from the accessibility tree.
**Fix:**
```html
<div class="terminal-bar" aria-hidden="true">
  <span class="terminal-dot red"></span>
  <span class="terminal-dot yellow"></span>
  <span class="terminal-dot green"></span>
</div>
```

---

### IN-02: Missing `og:image` meta tag

**File:** `docs/index.html:8-11`
**Issue:** Open Graph metadata has `og:title`, `og:description`, `og:type`, and `og:url`, but no `og:image`. Social platform link previews (Twitter/X, Discord, Slack) will render without an image, showing a plain text card. For a project landing page this reduces click-through quality on social shares.
**Fix:** Add an og:image pointing to a project logo or screenshot:
```html
<meta property="og:image" content="https://arbstr.com/og-image.png">
```

---

### IN-03: Missing canonical link tag

**File:** `docs/index.html` (head section)
**Issue:** No `<link rel="canonical">` is present. Without it, if the page is served from multiple URLs (e.g., with and without `www.`, via GitHub Pages domain and custom domain simultaneously), search engines may index duplicate versions and dilute page rank.
**Fix:**
```html
<link rel="canonical" href="https://arbstr.com">
```

---

### IN-04: Google Fonts preconnect missing `dns-prefetch` fallback

**File:** `docs/index.html:12-13`
**Issue:** `<link rel="preconnect">` is used for `fonts.googleapis.com` and `fonts.gstatic.com`, which is correct for modern browsers. Older browsers that do not support `preconnect` will not benefit. Adding a `dns-prefetch` hint as a fallback costs nothing and improves load time on legacy browsers.
**Fix:**
```html
<link rel="dns-prefetch" href="https://fonts.googleapis.com">
<link rel="dns-prefetch" href="https://fonts.gstatic.com">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
```

---

_Reviewed: 2026-04-10_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
