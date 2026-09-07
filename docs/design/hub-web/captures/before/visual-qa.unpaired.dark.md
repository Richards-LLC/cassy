# Visual QA — FAIL

**Summary:** FAIL · 2 finding(s) · 0 informational · 0 allowlisted · 2 screenshot(s)

Schemes: dark  
Viewports: desktop (1280×800), phone (390×800)

## Findings

- **javascript-disabled-loss** — `body` — content-requires-javascript (http://127.0.0.1:4173/commander/ · dark · desktop)
- **javascript-disabled-loss** — `body` — content-requires-javascript (http://127.0.0.1:4173/commander/ · dark · phone)

## Informational checks

None.

## Screenshots

- [127-0-0-1-4173-commander-dark-desktop.png](127-0-0-1-4173-commander-dark-desktop.png) — http://127.0.0.1:4173/commander/ · dark · desktop
- [127-0-0-1-4173-commander-dark-phone.png](127-0-0-1-4173-commander-dark-phone.png) — http://127.0.0.1:4173/commander/ · dark · phone

## Method

Headless Chromium rendered each URL under the requested color schemes and viewports. Text nodes were checked for effective WCAG contrast, clipping, overlap, visibility, viewport escape, and fixed-size truncation.
