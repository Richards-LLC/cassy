# Petrastella design language

The house visual language for every surface Petrastella ships: HTML reports, dashboards, product
pages, docs, and application UI. `design-tokens.json` beside this file is the machine-readable
twin; the values here and there are identical by test. A project inherits this language through
the `design-spec` skill and overrides it in its own `DESIGN.md`. A neutral grey-and-blue palette
is a documented fallback for white-label work, never the default.

The name is the brief: *Petra* — cut sandstone, warm, ruled, permanent; *Stella* — one bright
point that tells you where to look. Warm paper, one indigo mark, and typography that argues.

## 1. Type

One serif for the argument, one sans for the reading, one mono for the numbers. All three are
system stacks: no web font ever loads.

| Role | Stack | Where it appears |
| --- | --- | --- |
| display | Iowan Old Style, Palatino Linotype, Palatino, Book Antiqua, Georgia, Times New Roman, serif | verdict sentence, page title, pull-quotes; regular weight only, italic allowed |
| body | Inter, ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif | prose, headings, captions, controls |
| mono | JetBrains Mono, IBM Plex Mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace | ledger numbers, timestamps, eyebrows, provenance, hero numbers, code |

Scale: base 17px, ratio 1.25.

| Step | Size / line | Weight, family, treatment |
| --- | --- | --- |
| eyebrow | 12 / 16 | 600 mono, uppercase, tracking .08em |
| caption | 14 / 20 | 400 body |
| body | 17 / 26 | 400 body, measure 68ch |
| lede | 21 / 30 | 400 body |
| heading | 27 / 32 | 600 body, tracking −.01em |
| title | 34 / 38 | 400 display |
| verdict | clamp(32px, 4.6vw, 52px) / 1.08 | 400 display, tracking −.015em, max 22ch |
| hero-number | clamp(56px, 8vw, 96px) / 1 | 500 mono, tabular, tracking −.03em |
| ledger | 15 / 22 | 400 mono, tabular |

Rules that make the type read as ours:

- A verdict sentence is set in the serif, never bolded, never all-caps, at most 22 words. Emphasis
  inside it is italic or the indigo mark, not weight.
- Headings are sans 600; the serif is reserved for sentences that state a conclusion. If a serif
  line does not state a conclusion, set it in sans.
- Numbers a reader will compare are mono tabular and right-aligned. A standalone hero number is
  mono at hero-number size with its unit in caption type, not glued to the digits.
- Body measure never exceeds 68ch. Wide screens get a margin column, not wider prose.

## 2. Space and layout

4px base. Steps: 4, 8, 12, 16, 24, 32, 48, 64, 96. Sections breathe at 48–64; the hero at 96.
Container 1120px, gutter 24px. Breakpoints: phone 480, tablet 820, wide 1080.

At and above wide, the page has a 240px margin column on the right for marginal notes and figure
annotations; below wide, a marginal note renders inline directly under the paragraph or figure it
annotates. The margin column is empty by default and earns a note only when a reader would
otherwise have to infer something.

Elevation is none. Nothing on the page floats; hierarchy comes from type step, rule weight, and
the sandstone hero surface. The only shadow is the overlay shadow on dialogs and toasts.

Radius: 4px chips, 8px panels, 0 for tables and ledgers — a ledger is ruled, never boxed.

Containers that hold text follow the `container` rules in the tokens: no fixed height on a
text-bearing box unless the same rule declares its overflow strategy (scroll inside, ellipsis with
a title, or a clamp with a visible affordance); `overflow: hidden` only on decorative surfaces
with no text; tables and ledgers wider than their column scroll inside themselves; at least 12px
between a border and the glyphs it encloses; every text node wraps or scrolls at 390px with no
page-level horizontal scroll. Clipped text, a border through a glyph, and overlap are defects
the critique rubric scores as zero, whatever else the page does well.

## 3. Color

Semantic roles in light and dark. Every value below is the token; a project changes a value by
overriding the token in `DESIGN.md`, never by hardcoding a hex in a rule.

| Role | Light | Dark | Intent |
| --- | --- | --- | --- |
| bg | #F7F4EE | #12141A | warm stone paper; page background |
| surface | #FFFFFF | #191C24 | panels, ledgers, figure surfaces |
| surface-hero | #F1E7DD | #241E1E | Petra sandstone tint; verdict hero and pull-quotes only |
| line | #DAD3C7 | #2B3040 | hairlines, ledger rules, axes |
| line-strong | #8F8371 | #6B7390 | verdict rule, ledger sum rule |
| ink | #1B1D24 | #E9E6E0 | text (dark ink is a warm white, never #FFF) |
| ink-muted | #5A5F6E | #A3A7B4 | captions, provenance, axis labels |
| **verdict** | #2E3A9F | #A9B3FF | Stella indigo: the decisive mark in the hero figure, the verdict rule, the one highlighted row |
| verdict-soft | #DDE1F7 | rgba(169,179,255,.16) | band behind the decisive interval |
| **evidence** | #5A5F6E | #A3A7B4 | ledgers, annotations, marginal notes — the same value as ink-muted, on purpose: evidence is quiet |
| **action** | #2E3A9F | #A9B3FF | links, buttons, focus ring; shares the verdict hue |
| good | #226845 | #5FC492 | status only |
| **warning** | #7F5504 | #E2B14D | status only |
| danger | #B3261E | #EF7B72 | status only |

Named intents and their rules:

- **verdict** marks exactly one thing per figure. Two indigo marks in one chart means the chart has
  two arguments; split it.
- **evidence** is deliberately the muted ink. Evidence surrounds the verdict; it does not compete.
- **action** shares the verdict hue because a page has one accent. The two never share a figure:
  a chart carries no links, and a control rail carries no verdict mark.
- **status** colors (good, warning, danger) appear only when something *is* good, at risk, or
  failing, always with a label or sign, never as a series color and never as decoration.
- Contrast is stated, not assumed. Sanctioned text-on-surface pairs and their ratios live in
  `design-tokens.json` under `color.pairs`; the minimum across both schemes is 5.06:1 and no
  pair may fall below 4.5:1. Light: ink on bg 15.33, ink-muted on bg 5.8, ink-muted on
  surface-hero 5.22, verdict on bg 8.61, good on good-tint 5.3, warning on
  warning-tint 5.19, danger on danger-tint 5.06. Dark: ink on bg 14.78, ink-muted on bg 7.67,
  verdict on bg 9.27, danger on danger-tint 5.58. `line` (1.35:1) is a hairline and never carries
  meaning; `line-strong` clears 3:1 on both backgrounds for the rules that do.
- A rule that sets a background sets its foreground on the same line. White-on-white and
  ink-on-ink come from inherited colour meeting a new surface; the tokens make the pair explicit.

Print is always the light set on white, with the sandstone tint dropped.

## 4. Chart grammar

Marks, in order of preference: dot, bar, line, slope, dumbbell, waffle. A pie, gauge, or
radar never appears. Stroke 1.5px; hairline 1px in `line`; the decisive mark 2.5px in `verdict`.

Color by series uses a fixed order, and a series keeps its slot across every figure in a document:

| Slot | Light | Dark |
| --- | --- | --- |
| 1 | #4C5CA8 indigo | #7083D1 |
| 2 | #9D433B rose | #C86A60 |
| 3 | #0095A0 teal | #00A5B4 |
| 4 | #693F88 plum | #8C52B6 |
| 5 | #5C6C00 olive | #819334 |
| neutral | #6B7280 slate | #9AA1AF |

Slot 2 rose sits close to `danger` in dark mode; a figure that carries a danger status skips slot 2
and takes slot 3. The series slots share the verdict, rose and teal hues but sit in the chart lightness band
(OKLCH L 0.43–0.77 light, 0.48–0.67 dark) with chroma ≥ 0.10, so `verdict` itself is never a series color.
`neutral` (slate) is not a slot: it is the labelled other/rest bucket and reads as gray by design. Every slot,
the 3-slot subset and the full set pass the `cas-dataviz` `validate_palette.js` checks on `surface` and `bg` in
both schemes (receipts: docs/factory, task cas-fd80). Magnitude is a single-hue indigo ramp (five steps in
the tokens). Polarity is danger → line → good with the sign always printed. Validate any other subset with the
`cas-dataviz` palette script against the surface in use.

Variance encodings are by fill, not only by hue: actual = solid; plan = 1.5px outline, no fill;
forecast = 45° hatch at 4px pitch; estimate = dashed 4 3 with an 18%-alpha interval band; missing
= a gap in the line and a hollow marker labelled n/a. A variance is drawn as the variance (a
dumbbell, a slope, a signed bar from zero), never as two absolute bars the reader must subtract.

Every figure states its claim as its title, its population, window, unit, and source beneath it,
and carries a text alternative plus the numbers in a real table.

## 5. Motion

Chrome (hover, focus, toggle) 120ms; reveal (panel, details) 200ms; easing cubic-bezier(.2,0,0,1).
Nothing longer, nothing looping, no counters that count up, no animation that carries
information. `prefers-reduced-motion: reduce` removes every transition.

## 6. Signature components

**Verdict hero.** Eyebrow in mono (report type · date · audience) → the verdict sentence in the
display serif → the decisive figure beside it on wide screens and below it on narrow → a 3px
`verdict` rule → one line of provenance in mono `ink-muted`. Surface is `surface-hero`. Nothing
else sits above the fold: no KPI card row, no table of contents, no logo band. At 1280×800 and at
390×844 the sentence and the figure are both fully visible without scrolling.

**Evidence ledger.** A `<table>` styled as a ledger: caption above in body type stating what is
counted; header row in eyebrow type; hairline `line` rules between rows and nowhere else; numbers
in ledger type, right-aligned; sum row 600 weight above a `line-strong` rule; the last column
names the source of each row (query, file, commit, note). No zebra striping, no box, no rounded
corners, no icons in cells. The row the argument turns on gets a `verdict-soft` band and a
`verdict` left rule.

**Annotated timeline.** A vertical spine in `line-strong` with mono timestamps on the left,
events on the right, elapsed time between events printed on the spine (not left for the reader to
subtract), and the decisive event marked in `verdict` with its annotation drawn as a marginal note
connected by a hairline. Status colors mark only events that were failures or recoveries.

## 7. Inheriting and overriding

- `design-spec` seeds a new project's `DESIGN.md` from these tokens when the project has no token
  source of its own, and records which roles an existing project overrides when it has one.
- Override a role, keep its intent. A project may make `verdict` teal; it may not make `verdict`
  the same value as `danger`, and it may not give `warning` a job other than status.
- The type pairing (serif argument, sans reading, mono numbers) survives every override. A project
  may swap the specific stacks; a project that wants an all-sans page is choosing the neutral
  fallback and must say so in its `DESIGN.md` Overview.
- The neutral fallback is: `bg` #F5F6F8 / #0F1117, `surface` #FFFFFF / #161A24, `verdict` and
  `action` #2F5FD8 / #7AA2FF, status trio unchanged, display family = body family. It exists for
  clients whose brand must lead; it is never chosen for lack of a decision.
