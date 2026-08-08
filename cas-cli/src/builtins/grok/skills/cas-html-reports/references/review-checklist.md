# Pre-commit review checklist

Run this before committing the pair. Anything unchecked is a defect, not a nitpick. Eleven dimensions.

## 1. Content fidelity

- [ ] Every section, finding, number, and caveat in the markdown appears in the HTML.
- [ ] No conclusion was softened, sharpened, or invented during rendering.
- [ ] The HTML links to its markdown source by relative path.
- [ ] Both files are staged in the same commit.

## 2. Information architecture

- [ ] The conclusion is the first thing on the page.
- [ ] The required sections for this report type are present, in the prescribed order.
- [ ] An overview table appears before the detail.
- [ ] The depth order matches the audience (executive: detail relegated, methodology last).

## 3. Semantic HTML

- [ ] One `<h1>`; heading levels descend without skipping.
- [ ] Tabular data is in `<table>` with `<thead>`, scoped `<th>`, and a `<caption>`.
- [ ] Landmarks present: `<header>`, `<main>`, `<footer>`.
- [ ] `lang`, `charset`, viewport, and a descriptive `<title>` with the report date.

## 4. Responsive layout

- [ ] Readable at 360 px wide with no horizontal scroll except inside wide tables.
- [ ] Wide tables scroll within their own container, not the page.
- [ ] Charts scale with `viewBox` and `preserveAspectRatio`, not fixed pixel widths.
- [ ] For an executive brief: the hero, KPI cards, and one chart fit one screen at 1280×800.

## 5. Accessibility

- [ ] Contrast ≥ 4.5:1 for text, ≥ 3:1 for graphical objects, in light and dark.
- [ ] No meaning conveyed by color alone anywhere — status, variance, series, all labeled.
- [ ] Every chart has `role="img"`, a title/`aria-label`, a text summary, and its numbers in a table.
- [ ] Keyboard-only pass reaches every control; focus is always visible.
- [ ] Interactive widgets carry correct ARIA roles and state.

## 6. Interaction quality

- [ ] **JS disabled: reload — no content is lost.** Tabs become stacked sections; details render expanded.
- [ ] Tabs support Arrow keys, Home, and End; the selected tab is programmatically marked.
- [ ] No animation that conveys information; `prefers-reduced-motion` respected.

## 7. Dependency policy

- [ ] Zero external requests. Grep the file for `http://`, `https://`, `src=`, `@import`, `cdn` — nothing
      loads at render time (links a reader may *click* are fine).
- [ ] All CSS in one inline `<style>`; all JS in one inline `<script>`; no sibling asset files.
- [ ] System font stack only.
- [ ] File under ~500 KB.

## 8. Copyability

- [ ] Select a table and paste it into a spreadsheet — columns land correctly.
- [ ] Commands and code copy without stray line numbers or wrapping artifacts.
- [ ] Numbers paste as parseable numbers.

## 9. Print readiness

- [ ] Print preview: every panel and `<details>` is expanded; nothing is hidden.
- [ ] No clipped charts; no table split mid-row; headers repeat across pages.
- [ ] Link targets are shown.
- [ ] Prints legibly in grayscale — scenario and status remain distinguishable by fill/pattern/label.

## 10. Visual emphasis

- [ ] The one thing the reader must take away is the most visually prominent thing on the page.
- [ ] Emphasis is used sparingly enough to still mean something.
- [ ] Charts follow the presentation rules: zero baselines, shared sibling scales, no dual axes, no 3-D.
- [ ] Variance is shown as variance, not as two values side by side.
- [ ] Scenario fills are correct: actual solid, plan outlined, forecast hatched.

## 11. Provenance and source of truth

- [ ] Every figure names its source and extraction time.
- [ ] The report names the commit, data window, and the exact queries or commands used.
- [ ] Estimates are labeled as estimates, with their method.
- [ ] For client-facing reports: no internal ticket IDs, team names, tool names, or process narration
      anywhere in the HTML.

## The two-minute version

If you check nothing else: **conclusion first, JS off and nothing is lost, no external requests, print
preview is clean, every number has a source, and the markdown says exactly what the HTML says.**
