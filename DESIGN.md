---
version: alpha
name: Cassy
description: The brand every Cassy report, release report and HTML document wears; Commander's Glass tokens and faces, with no palette of its own.
inherits: hub-web/DESIGN.md
source: [hub-web/public/cassy-tokens.css, hub-web/src/fonts.css]
brand-mark: hub-web/public/favicon.svg
maps:
  bg: "--bg-root"
  surface: "--bg-panel"
  surface-hero: "--bg-raised"
  card: "--bg-raised"
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
  hero-band: "--look-aurora"
  glass-panel: "--look-glass"
  glass-panel-strong: "--look-glass-strong"
  glass-edge: "--look-glass-edge"
  glass-blur: "--look-blur"
  glass-shadow: "--look-shadow-float"
  lift: "--lift"
  emphasis: "--look-send"
  on-emphasis: "--you-fg"
  radius-panel: "--glass-radius"
  radius-tile: "--radius-pane"
  radius-chip: "--radius-card"
---
## Overview

This is the repo-level brand that `cas-html-reports`, `cas-release-report`, `cas-ui-craft` and `cas-dataviz` read as "the project's DESIGN.md". A Cassy document should be recognisable as Commander: a still violet-to-teal aurora, frosted glass chrome, opaque reading cards, the display serif verdict, Inter prose and JetBrains Mono numbers.
It defines no colours. The palette is the hub's, documented in [hub-web/DESIGN.md](hub-web/DESIGN.md) and published as `hub-web/public/cassy-tokens.css` (the token blocks of `hub-web/src/tokens.css` and `hub-web/src/glass.css`, hash-versioned). `hub-web/src/fonts.css` carries Commander's Inter and JetBrains Mono faces, and `brand-mark` is the Cassy tile.
`source` lists what a document inlines: the token CSS whole, and each `@font-face` with its `.woff2` as a data URI. `maps` names the variable behind each document role. A renderer declares each role as `--role: var(--variable)` and skips one already named like its variable. The release-report renderer does exactly this. When the hub changes a token, `npm run tokens` regenerates the file and every new document follows it. `hub-web/src/tokens.test.ts` fails if this frontmatter gains a colour literal or names a variable the hub does not ship.
Lint (`npx @google/design.md lint DESIGN.md`) is clean: no colours means no contrast pairs and no orphaned tokens. The contrast figures below were measured against the hub values.

## Colors

- Use the role names in `maps`. `verdict` and `action` are the same violet: one emphasis per figure, as in the hub.
- On screen the page sits on the aurora (`hero-band`, painted on a fixed layer behind everything); chrome is `glass-panel`; reading cards are `surface` (opaque) and inner tiles `card` (`--bg-raised`).
- Print is the light set on white (`surface`). The aurora survives only as the hero band, printed with `print-color-adjust: exact`; frosted panels and shadows do not print.
- Evidence is quiet: `ink-muted` for captions, provenance and axis labels (8.2:1 on `card`).
- `emphasis` is the hub's Send gradient with `on-emphasis` white (4.7:1 or better across it): one lead figure per document, never a row of them.
- Status roles (`good`, `warning`, `danger`) mark status only, never a decorative fill. Charts take `cas-dataviz` encodings: the decisive mark in `verdict`, other marks in `series-neutral` (grey against violet in both schemes), gridlines in `line`.

## Typography

- Verdict and title: `font-display`, 400, with the emphasised phrase in `verdict` italic. The lockup wordmark is `font-display` 500.
- Prose: `font-body` (Inter) at 16px on screen, 10pt in print, measure 68ch. Headings 600, never above it.
- Eyebrows and section numbers: `font-mono` 600, 12px, uppercase, .08em tracking, `ink-muted`.
- Figures: `font-mono` 500, tabular; ledger columns right-aligned.

## Layout

- The hub's 4px grid. One centred column up to 1152px with a 16px gutter (8px at 390px); cards stack 16px apart (8px on a phone).
- The first screen at 1280×800 and 390×844 holds the verdict and its figure, nothing above them but the lockup line.
- In print, sections flow continuously (`break-before: auto`) and nothing breaks inside a tile, card, row or figure.

## Elevation & Depth

- Screen: aurora (level 0) → glass hero (`glass-panel`, `glass-blur`, `glass-shadow`, a 1px `glass-edge` drawn inset) → opaque section cards with `lift` → inner tiles as a fill step (`card`), no extra shadow.
- Print is flat: no blur, no shadow; depth is the fill step alone.

## Shapes

- `radius-panel` (22px) for the hero and section cards, `radius-tile` (8px) for tiles and the figure card, `radius-chip` (4px) for note panels and code wells.
- A Was → Now pair takes Commander's bubble shapes: Was is the supervisor's (`radius-tile` on the inner corners, `radius-panel` outside), Now the operator's mirror image.
- Rules: `line` 1px between rows and themes, `line-strong` for totals and the closing folio, a 3px left rule on note panels and code wells.

## Components

- **Hero.** Lockup (the `brand-mark` tile and product name) with the eyebrow, then the verdict and the change map in one glass panel; the map sits on a `glass-panel-strong` card.
- **Section card.** Eyebrow number ("02 / 06") above a 600 heading, on an opaque `surface` card.
- **Glance tiles.** The first measure is the lead tile on `emphasis`; the rest are `card` tiles. The counting note below is a note panel (`card`, 3px `line-strong` left rule).
- **Was → Now.** Was on `card` in `ink-muted`; Now on `surface` with a 2px inset `action` edge, as Commander marks the open conversation. Each label is a mono eyebrow, Now's in `verdict`.
- **Ledger.** Mono uppercase header, `line` row rules, `line-strong` closing rule, tabular figures.
- **Code well.** `card` fill, 3px `verdict` left rule, mono; install commands large enough to copy by eye.
- **Folio.** A closing line inside the last card (product, version, source and print links in mono `ink-muted`, above a `line-strong` rule) and, in print, a running footer with the document title and page "n / N" in the `font-mono` stack (the first face installed on the printing machine) and `ink-muted`.

## Do's & Don'ts

- ✅ Inline the `source` files and use `var(--role)` everywhere.
- ❌ Never type a hex value into a document stylesheet or copy the hub's colours into a second palette; a document that needs a colour the hub lacks gets a hub token first.
- ✅ Keep one violet emphasis per figure and one lead tile per document.
- ❌ Never put text straight on the aurora outside the hero's glass, or print `glass-panel`, `backdrop-filter` or shadows.
- ✅ Write every change as Was → Now in plain sentences.
- ❌ Never recolour the mark or set the wordmark in anything but `font-display`.
