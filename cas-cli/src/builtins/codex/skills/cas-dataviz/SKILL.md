---
name: cas-dataviz
description: Use when a Cassy report, issue, or note needs a static, self-contained figure (inline SVG plus its data table) or a document is becoming text-dense. Interactive or library-rendered charts belong to the bundled `dataviz` skill.
metadata:
  managed_by: cas
---

# Figures that show the argument

**When both load, this skill governs committed Cassy artifacts; the bundled `dataviz` skill governs live or interactive charts.** Form vocabulary and the critique rubric come from `cas-ui-craft`; the report contract comes from `cas-html-reports`. This skill is how one figure gets built.

## Procedure

1. State the message as a claim-title; name the population, time window, unit, and source.
2. **Choose the form, and say why.** Take it from the one form table, `cas-ui-craft/references/form-vocabulary.md`, by the reader's task — a ledger, slope, dot/waffle plot, annotated timeline, small multiples, or a table when exact lookup, many categories, or auditability is primary — and write the one-line reason in the concept brief (`<basename>.brief.md`, committed beside the markdown).
3. Format numbers before drawing: compact figures only where scanning benefits (`12.4K`, `$4.2M`); unit in the title, subtitle, or axis; one precision per column; thousands separators; never mix percent, fraction, and percentage-point changes.
4. Show uncertainty when the data has it: intervals or bands for estimates, n for samples, missing-data marks, and a plainly labelled baseline or forecast (forecast is hatched, plan is outlined, actual is solid). No false precision from rounded or partial data.
5. Draw quiet structure: direct labels for the endpoint, extreme, or annotated insight; hairline axes and grid; thin marks; white space. A legend supports two or more series and never substitutes for annotation. Expression lives in the annotation and the typographic hierarchy, not in effects.
6. Assign color last, from the design tokens' semantic roles: `evidence` (quiet) for measured marks, `verdict` for the one mark the argument rests on, `good`/`warning`/`danger` for status with a label or shape beside it; `color.magnitude` (one hue, light-to-dark) for quantity; `color.series` in fixed slot order for identity; `color.polarity` (warm/cool with a neutral midpoint, sign always printed) for direction. Color follows an entity, never its rank after filtering. Take the tokens from the project's `DESIGN.md` when it exists, otherwise paste `design-spec/references/tokens.css`.
7. Validate any categorical palette — including a subset of `color.series` — with `node <skills-dir>/cas-dataviz/scripts/validate_palette.js "#hex,#hex" --surface "#FFFFFF"` (light) or `--mode dark --surface "#191C24"`; record the command in provenance. Do not eyeball contrast or separability.
8. Add a text alternative (`role="img"` plus `<title>`/`<desc>` or `aria-label` carrying every value) and an adjacent real `<table>` twin. In a report, the figure's caption states its source and extraction time.
9. **Visually verify the rendered artifact — mandatory.** Screenshot with headless Chrome at a desktop width and a phone-class `390×844` viewport, in light and dark; for a report, also render print/PDF. Look for label collisions, overflow and clipping, contrast in situ, broken layout, and the 30-second argument test. Grepping HTML for expected strings or tags is **not** visual verification: it proves markup exists, not that a human can read it.
10. Score the figure with the `cas-ui-craft` critique (`references/critique-rubric.md`, including its visual-QA receipt) and append the table to the brief. A public-surface figure ships at 4 or above on distinctiveness, fit, and hierarchy, with no mechanical defect.

**Done when** every item in [the quality checklist](references/quality-checklist.md) is yes and the brief carries the critique.

## Start with the message, not the chart

Write one sentence that says what the reader should learn and make it the chart title. "Failures by class" names an axis; "Merge commits were the largest class today, twice the next largest" makes a claim the figure must prove. If no useful sentence emerges, use a table or keep investigating.

Put the evidence *on the figure*: annotate the decisive point, bar, interval, or threshold with a brief callout and a connector when necessary. A reader never infers the conclusion from a paragraph beside an unmarked plot. Use small multiples when the comparison is among several similarly shaped series; share scales and alignment, and annotate the panel where the finding lives. Never use two y-axes: split, facet, or index both series to a common baseline.

## Give dense documents visual rhythm

When quantitative or enumerable prose becomes a wall of text, convert the part readers must compare into a figure, a stat strip, a table, or a timeline. A long report leads with its hero figure, paces sections with figures and tables, attaches caveats as marginal notes beside the evidence they qualify, and reserves a pull-quote for the one sentence that must survive. **Acceptance test:** a reader gets the argument from the figures and claim-titles alone in 30 seconds.

## Cassy output contexts

For durable Cassy reports, use static inline SVG and CSS inside one self-contained HTML file: no charting library, CDN, build step, or external asset. Hover is an optional enhancement (CSS-only tooltips are fine); static legibility leads because a figure must survive GitHub embeds, PDF, and print. Use real `<table>` markup for the evidence twin, explicit provenance beneath the figure, and `@media print` rules that retain title, annotation, legend, and table without clipping.

GitHub issues and PRs need a compact static SVG or table with the same claim-title and provenance. Terminal-adjacent Markdown should use a small aligned table or Unicode sparkline; do not simulate a dense dashboard in text.

## Guardrails

- No dual axis, decorative rainbow, color-only meaning, tooltip-only value, or number on every point.
- No extra colors to solve too many series: aggregate, facet, small-multiple, or table instead.
- No gradients, glows, drop shadows, or animated counters; a figure earns attention with its claim, not its finish.
- Texture only for print, forced-colors, or an explicit accessibility option, never as decoration.
- Large standalone numbers use proportional figures; aligned table columns and axis ticks use tabular figures.

See [the worked example](examples/send-backs-dot-strip.html) with its [sidecar](examples/send-backs-dot-strip.why.md) for a self-contained SVG figure whose form was chosen for a reason; [the design review](references/design-review.md) records why this skill departs from the bundled one.
