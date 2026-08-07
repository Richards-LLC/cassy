# The invariant technical contract

Every report HTML obeys all of this, regardless of type, audience, or domain. If a rule below is
violated, the artifact is not shippable — fix it before committing.

## 1. One file, zero dependencies

- **Exactly one `.html` file.** No sibling CSS, JS, font, or image files.
- **No network at render time.** No CDN links, no web fonts, no remote images, no analytics, no
  `fetch()`. Open it offline; it must be complete.
- **No frameworks, no build step.** No React, no Tailwind CDN, no bundler, no template engine that has
  to run before a human can read it. Hand-written HTML and CSS.
- **Fonts**: system font stack only.
- **Images**: inline SVG, or a small data URI when a raster is unavoidable. Prefer drawing the chart in
  SVG over embedding a screenshot — a screenshot is not copyable, not accessible, and not printable at
  arbitrary size.
- **Size**: keep it under ~500 KB. If you are past that, you are embedding raw data that belongs in a
  linked data file next to the markdown.

## 2. JavaScript is progressive enhancement only

- Vanilla JS, inline in one `<script>`, no dependencies.
- **Everything must be reachable with JS disabled.** Test it: disable JavaScript, reload, and confirm
  no content is lost.
- Tabs: render the panels as ordinary stacked sections in the no-JS state; the script converts them
  into tabs. Never `display: none` a panel from static CSS that only JS can undo.
- Collapsibles: use `<details>`/`<summary>`, and ship them with the `open` attribute **in the markup** so
  the no-JS state is fully expanded. JS may collapse them for screen reading; a `beforeprint` listener
  re-opens them. CSS alone cannot force a closed `<details>` open for print — do not try.
- Keyboard support is required for anything JS makes interactive: tabs implement Arrow keys, Home/End,
  and correct `role="tab"` / `aria-selected` / `aria-controls` wiring, with `tabindex` roving.
- No JS-computed content. Every number is in the markup, not produced at runtime.

## 3. Semantic and accessible HTML

- `<!DOCTYPE html>`, `<html lang="en">`, `<meta charset="utf-8">`, a viewport meta, and a `<title>` that
  names the report and its date.
- One `<h1>`. Heading levels descend without skipping. Headings are structure, not styling.
- Tabular data uses `<table>` with `<thead>`, `<th scope="col">` / `<th scope="row">`, and a `<caption>`.
  Never a grid of `<div>`s for data.
- Landmarks: `<header>`, `<main>`, `<section>`, `<footer>`. A skip link when the report is long.
- Contrast ≥ 4.5:1 for body text and ≥ 3:1 for graphical objects, in **both** light and dark rendering.
- Visible focus outlines. Never `outline: none` without a replacement.
- **Never encode meaning by color alone.** Pair color with a label, a pattern, an icon shape, or a sign.
- Every chart carries: a `<title>` (or `aria-label`) inside the SVG, `role="img"`, a one-sentence text
  summary of what it shows, and the underlying numbers in a real table (visible or in a `<details>`).
- Respect `prefers-reduced-motion`; there should be little motion to begin with.

## 4. Theme

- Support light and dark via `prefers-color-scheme`. Define colors as CSS custom properties once and
  reference them everywhere — no hard-coded hex values scattered through rules.
- The palette is neutral and restrained: one accent, one warn, one bad, one good, plus greys. Data
  colors come from the palette in a fixed order so the same series gets the same color in every figure.
- Print rendering is always light regardless of the screen theme.

## 5. Print

A `@media print` block is mandatory:

- Expand every tab panel and every `<details>`; nothing may be hidden on paper.
- Show link targets (`a[href^="http"]::after { content: " (" attr(href) ")" }`).
- Avoid page breaks inside tables rows, cards, and figures (`break-inside: avoid`).
- Repeat table headers across pages (`thead { display: table-header-group }`).
- Drop backgrounds that waste ink; keep borders that carry meaning.
- Fit the content to a portrait page width; charts must not clip.

## 6. Provenance

Every number in the report must be traceable without asking you.

- **Per figure**: a caption or footnote naming the source — the query, script, file path, log window,
  commit SHA, or upstream system, plus the extraction time.
- **Per report**: a provenance section (footer for practitioner audiences, final section for executive
  ones) with the markdown source path, the commit the analysis was run against, the data window, and
  the commands or queries used verbatim.
- **Estimates are labeled as estimates**, with the method. A modeled number and a measured number must
  never look alike.
- Rounding is stated when it matters; do not present a rounded number and a precise one in the same
  column.

## 7. Copyability

- Tables are real tables so a reader can select and paste into a spreadsheet.
- Do not inject characters into numbers that break parsing (no non-breaking spaces inside digit runs
  where a plain space or nothing will do).
- Code and commands sit in `<pre><code>` and copy cleanly, without line numbers baked into the text.

## 8. Source-of-truth preservation

- The HTML contains everything the markdown contains. Rendering must not drop sections, soften a
  conclusion, or lose a caveat. If the HTML says less than the markdown, it is wrong.
- The HTML links to its markdown source by relative path.
- Regenerate the HTML in the same commit as any markdown edit. A stale artifact is a correctness bug,
  not a cosmetic one.

## Anti-patterns

Each of these has been observed to make a report worse; none are stylistic preferences.

- **HTML for a two-paragraph answer.** Ceremony around nothing. Markdown was sufficient.
- **A CDN link "just for the chart library."** It breaks offline, and it breaks the day the CDN changes.
- **A screenshot of a table.** Not copyable, not searchable, not accessible, unreadable when printed.
- **JS-only content.** Content that vanishes with JS disabled did not survive the reader's browser.
- **Decorative dashboards.** Gradients, glass effects, animated counters, hero images, and gauges that
  encode one number in 400 pixels. Ink that carries no data is noise.
- **Two absolute bars where a variance was the point.** Make the reader subtract and they will misread it.
- **Color-only status.** Red/green with no label fails for a colorblind reader and for a printout.
- **Rotating metrics between editions.** A recurring report whose KPI set changes cannot be trended.
- **Truncated axes and dual axes.** Both manufacture conclusions the data does not support.
- **Numbers with no source.** An untraceable number is an unverifiable claim.
- **Conclusion at the bottom.** The reader who stops after the first screen — most of them — got nothing.
- **Internal jargon in an external deliverable.** Ticket IDs, agent names, and process narration are
  a hygiene failure, not a detail.
- **HTML edited by hand after generation.** Now the two files disagree and the markdown quietly lost.
