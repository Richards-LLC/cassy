---
version: alpha
name: Cassy
description: The brand every Cassy report, release report and HTML document wears; Commander's Glass tokens plus the print roles a document needs, with no palette of its own.
inherits: hub-web/DESIGN.md
source: [hub-web/DESIGN.md, hub-web/public/cassy-tokens.css]
maps:
  bg: "--bg-root"
  surface: "--bg-panel"
  surface-hero: "--bg-raised"
  line: "--line-subtle"
  line-strong: "--line-strong"
  ink: "--ink"
  ink-muted: "--ink-mid"
  verdict: "--color-verdict"
  verdict-soft: "--bg-active"
  evidence: "--ink-mid"
  action: "--color-action"
  primary: "--color-action"
  good: "--state-ok"
  warning: "--state-warn"
  danger: "--state-crit"
  series-neutral: "--color-series-neutral"
  font-display: "--font-display"
  font-body: "--font-ui"
  font-mono: "--font-mono"
  role.masthead: "band --look-aurora, rule --look-send, rule-height --rule-hero, gap --space-6"
  role.eyebrow: "color --ink-mid, font --font-ui, size --fs-xs, weight --weight-semibold, tracking --tracking-label"
  role.title: "color --ink, font --font-display, weight --weight-regular"
  role.lede: "color --ink-mid, font --font-ui, size --fs-lg, weight --weight-regular"
  role.meta-strip: "label --ink-mid, value --ink, font --font-ui, size --fs-meta, rule --line-subtle"
  role.section-number: "fill --color-verdict, text --accent-fg, font --font-mono, weight --weight-semibold, radius --radius-pill"
  role.kpi-tile: "fill --bg-raised, edge --line-subtle, figure --ink, figure-font --font-mono, figure-weight --weight-medium, caption --ink-mid, radius --radius-pane, padding --space-4"
  role.kpi-tile-lead: "fill --look-send, figure --you-fg, caption --you-fg, radius --radius-pane, padding --space-4"
  role.ledger: "rule --line-subtle, sum-rule --line-strong, numbers --font-mono, size --fs-md, total-weight --weight-semibold"
  role.was-now-card: "fill --bg-panel, edge --line-subtle, radius --radius-pane, padding --space-4, was --ink-mid, now --color-verdict"
  role.caveat-panel: "fill --bg-raised, rule --state-warn, rule-width --rule-hero, text --ink, radius --radius-card"
  role.note-panel: "fill --bg-raised, rule --line-strong, rule-width --rule-hero, text --ink, radius --radius-card"
  role.ask-panel: "fill --look-ask, text --ask-fg, radius --radius-pane, padding --space-4"
  role.folio: "color --ink-mid, font --font-mono, size --fs-xs, rule --line-subtle"
---
## Overview

This is the repo-level brand that `cas-html-reports`, `cas-release-report`, `cas-ui-craft` and `cas-dataviz` read as "the project's DESIGN.md". It is Commander's look carried onto paper: violet Glass, the display serif title, Inter prose and JetBrains Mono numbers.
It defines no colours. The palette is the hub's, documented in [hub-web/DESIGN.md](hub-web/DESIGN.md) and published as `hub-web/public/cassy-tokens.css` (the custom-property blocks of `hub-web/src/tokens.css` and `hub-web/src/glass.css`, hash-versioned). A document pastes that file's blocks into its one `<style>` and uses the variables in `maps`. When the hub changes a token, `npm run tokens` regenerates the file and every new document follows it. `hub-web/src/tokens.test.ts` fails if this frontmatter gains a colour literal or names a variable the hub does not ship.
This file adds only what print surfaces need and the hub lacks: masthead, eyebrow, title and lede, meta strip, section number, KPI tile, ledger, Was → Now card, caveat and note panels, ask panel and folio. Their structure follows the operator's brand exemplar (the Pulse Card October update and score-transparency PDFs); their colours are the hub's.
Theme: documents render the light scheme (`<html data-scheme="light">`) on screen and in print; dark is for the app, not paper.
Release reports: `cas release report` currently reads `docs/design/design-tokens.json` (the house roles before Glass). Moving the renderer onto this file is separate work (cas-fba9).
Lint (`npx @google/design.md lint DESIGN.md`) is clean: no colours means no contrast pairs and no orphaned tokens to check; the contrast figures below were measured against the hub values.

## Colors

- Use the Petrastella role names in `maps`; each resolves to a hub variable. `verdict` and `action` are the same violet: one emphasis per figure, as in the hub.
- Page: `--bg-panel` (white) for the printed sheet, as the Petrastella rule has it (print is the light set on white); `--bg-root` only behind on-screen documents. `surface-hero` is `--bg-raised`, the lavender-white card; the aurora (`--look-aurora`) appears only in the masthead band.
- Evidence is quiet: `--ink-mid` for captions, provenance and axis labels (8.2:1 on `--bg-raised`).
- Status colours (`--state-ok`, `--state-warn`, `--state-crit`) mark status only: the meta-strip status value, a caveat rule, a regression in a ledger. Never a decorative tile fill.
- Charts take `cas-dataviz` encodings: the decisive mark in `--color-verdict`, comparison series in `--color-series-neutral`, gridlines in `--line-subtle`.

## Typography

- Title: `--font-display`, 400, 34/38px (the Petrastella title step); one per document.
- Lede: `--font-ui` `--fs-lg` 21px, `--ink-mid`, one sentence.
- Eyebrows (masthead line, section labels, Was/Now labels, meta labels): `--font-ui` `--fs-xs` 12px, 600, uppercase, `--tracking-label` .08em. A dot separates parts: "RELEASE · 20 OCTOBER 2026".
- Body: `--font-ui` `--fs-base` 14px / `--line-ui` 1.43, measure 68ch.
- Numbers: `--font-mono`, tabular (`font-variant-numeric: tabular-nums`); KPI figures 500 at 28–34px, ledger figures `--fs-md`. A count "251 of 253" keeps the denominator in the same face at caption size.
- Folio: `--font-mono` `--fs-xs`.

## Layout

- The hub's 4px grid (`--space-*`). Sections are separated by `--space-8`; tiles and cards by `--space-3`.
- One column of 68ch prose with full-width rows for tiles, ledgers and card rows; the page box (size, margins) belongs to the renderer.
- KPI tiles sit four to a row on desktop and print, two at 390px. Was → Now cards sit three to a row, one at 390px.
- Page one carries the masthead, the meta strip, the at-a-glance tiles and the first section; nothing breaks inside a tile, card or ledger row (`break-inside: avoid`).

## Elevation & Depth

- Paper is flat: no `box-shadow` and no blur in print. Depth is a fill step: page `--bg-panel` → tile/panel `--bg-raised` → lead tile `--look-send`.
- On screen a document may use the hub's `--lift` on tiles; never `--look-glass` (frosted panels need the aurora behind them and print as grey).
- Backgrounds that carry meaning (masthead band, lead tile, ask panel, section numbers) set `print-color-adjust: exact`.

## Shapes

- Tiles, Was → Now cards and the ask panel: `--radius-pane` 8px. Caveat and note panels: `--radius-card` 4px with a 3px (`--rule-hero`) left rule. Section numbers and status chips: `--radius-pill`.
- Rules: `--line-subtle` 1px for ledgers and the folio; `--line-strong` for a ledger's total row; the masthead rule is `--rule-hero` 3px of the `--look-send` gradient.
- Brand mark: `hub-web/public/favicon.svg` at 32px, inlined as a data URI; never recoloured (hub-web/DESIGN.md, Brand).

## Components

- **Masthead.** Mark and "Cassy" wordmark (`--font-display`, 500), then the eyebrow, title and lede on the `--look-aurora` band, closed by the gradient rule.
- **Meta strip.** Four label/value pairs in one row: Prepared for, Prepared by, Date, Status. Labels are eyebrows; the status value takes its state colour (FINAL in `--state-ok`).
- **Section header.** A filled `--color-verdict` circle with the section number in `--accent-fg` mono, then the section eyebrow and an optional context eyebrow ("LIVE SINCE FRI 17 OCT").
- **KPI tile.** Figure, then a caption of at most two lines. The one figure that answers the document's question is the lead tile: `--look-send` fill, white (`--you-fg`) figure and caption, 4.7:1 or better across the gradient. Other tiles are `--bg-raised` with a `--line-subtle` edge. Never more than one lead tile per row.
- **Ledger.** Mono tabular figures, right-aligned; a header row of eyebrows; `--line-subtle` row rules and a `--line-strong` total row in 600.
- **Was → Now card.** A context eyebrow (date or scope), a one-line heading in 600, then WAS (`--ink-mid` eyebrow) and NOW (`--color-verdict` eyebrow), each followed by one or two plain sentences. Every change in a report is one card.
- **Caveat panel.** `--bg-raised` with a 3px `--state-warn` left rule: what is not yet true, partial, or measured differently. A neutral aside uses the note panel (`--line-strong` rule).
- **Ask panel.** Decisions needed from the reader: the hub's ask card, `--look-ask` with `--ask-fg` text (8.5:1 or better), numbered items. At most one per document.
- **Folio.** Footer on every page: "Cassy · <document title> · <date>" left and "n/N" right, `--font-mono` `--fs-xs` `--ink-mid` above a `--line-subtle` rule.

## Do's & Don'ts

- ✅ Paste the token blocks of `hub-web/public/cassy-tokens.css` into the document and use `var(--…)` everywhere.
- ❌ Never type a hex value into a report stylesheet or copy the hub's colours into a second palette; a document that needs a colour the hub lacks gets a hub token first.
- ✅ Keep one violet emphasis per figure and one lead tile per row.
- ❌ Never use the Pulse Card colours (orange, blue, purple tiles); the exemplar defines structure only.
- ✅ Write every change as Was → Now in plain sentences.
- ❌ Never use `--look-glass`, `backdrop-filter` or shadows in print.
- ✅ Render the light scheme for documents, and set `print-color-adjust: exact` on meaningful fills.
- ❌ Never recolour the mark or render the wordmark in anything but `--font-display`.
