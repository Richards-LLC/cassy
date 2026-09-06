---
name: cas-dataviz
description: Use when a Cassy report, GitHub issue, or Markdown note needs a static, self-contained figure (inline SVG plus its data table, print-safe) or when a document is becoming text-dense. The harness-bundled `dataviz` skill owns interactive and library-rendered charts; this skill owns durable evidence artifacts.
managed_by: cas
---

# Figures that show the argument

Claude sessions may also carry a bundled `dataviz` skill; this Cassy skill is the canonical cross-harness companion. It keeps that skill's color discipline and fits Cassy's durable, self-contained HTML surfaces. Form vocabulary and the critique rubric come from `cas-ui-craft`; the report contract comes from `cas-html-reports`. This skill is how one figure gets built.

## Start with the message, not the chart

Write one sentence that says what the reader should learn and make it the chart title. "Failures by class" names an axis; "Merge commits were the largest class today, twice the next largest" makes a claim the figure must prove. If no useful sentence emerges, use a table or keep investigating.

Put the evidence *on the figure*: annotate the decisive point, bar, interval, or threshold with a brief callout and a connector when necessary. A reader never infers the conclusion from a paragraph beside an unmarked plot.

## Choose the form, and say why

Every figure in a report has a one-line reason in the concept brief (`<basename>.brief.md`, from `cas-ui-craft/references/concept-brief.md`, committed beside the markdown): the reader's task, the form, and why that form fits. "A table" needs a reason as much as a slope chart does. Choose from the reader's task, then take the least-ink form that does it:

| Reader's task | Prefer | Avoid |
| --- | --- | --- |
| See a claim next to what contradicts or supports it | ledger: aligned columns per row, a verdict stamp in the gutter | two tables the reader must reconcile |
| One current value | stat tile or hero number | one-bar chart |
| Compare magnitudes | sorted bar chart, or a dot/waffle plot when every unit matters | pie for close values |
| Show change over time | line; area only for one series | dual axes |
| Compare before/after or two states | slope or dumbbell | disconnected columns |
| Show when things happened relative to each other | annotated timeline | a table of timestamps |
| Show composition | stacked bar, usually horizontal | more than six pie slices |
| Show a matrix | heatmap plus values/table | rainbow scale |
| Preserve exact evidence or many classes | table, optionally paired with a chart | a chart carrying more than ~7 meanings |

Use small multiples when the comparison is among several similarly shaped series; share scales and alignment so differences, not chart furniture, carry the work, and annotate the panel where the finding lives. Never use two y-axes: split, facet, or index both series to a common baseline.

## Give dense documents visual rhythm

When quantitative or enumerable prose becomes a wall of text, convert the part readers must compare into a figure, a stat tile, a table, or a timeline. A long report leads with its hero figure, paces sections with figures and tables, attaches caveats as marginal notes beside the evidence they qualify, and reserves a pull-quote for the one sentence that must survive. **Acceptance test:** a reader gets the argument from the figures and claim-titles alone in 30 seconds.

## Procedure

1. State the message as a claim-title; name the population, time window, unit, and source.
2. Pick the form from the reader's task and write the reason. Prefer a table when exact lookup, many categories, or auditability is primary.
3. Format numbers before drawing: compact figures only where scanning benefits (`12.4K`, `$4.2M`); unit in the title, subtitle, or axis; one precision per column; thousands separators; never mix percent, fraction, and percentage-point changes.
4. Show uncertainty when the data has it: intervals or bands for estimates, n for samples, missing-data marks, and a plainly labelled baseline or forecast (forecast is hatched, plan is outlined, actual is solid). No false precision from rounded or partial data.
5. Draw quiet structure: direct labels for the endpoint, extreme, or annotated insight; hairline axes and grid; thin marks; white space. A legend supports two or more series and never substitutes for annotation. Expression lives in the annotation and the typographic hierarchy, not in effects.
6. Assign color last, from the design tokens' semantic roles: `evidence` (quiet) for measured marks, `verdict` for the one mark the argument rests on, `good`/`warning`/`danger` for status with a label or shape beside it; `color.magnitude` (one hue, light-to-dark) for quantity; `color.series` in fixed slot order for identity; `color.polarity` (warm/cool with a neutral midpoint, sign always printed) for direction. Color follows an entity, never its rank after filtering. Take the tokens from the project's `DESIGN.md` when it exists, otherwise from `design-spec/references/design-tokens.json`.
7. Validate any categorical palette — including a subset of `color.series` — with `node scripts/validate_palette.js "#hex,#hex" --surface "#FFFFFF"` (light) or `--mode dark --surface "#191C24"`; record the command in provenance. Do not eyeball contrast or separability. The validator ships in every harness mirror.
8. Add a text alternative (`role="img"` plus `<title>`/`<desc>` or `aria-label` carrying every value) and an adjacent real `<table>` twin. In a report, the figure's caption states its source and extraction time.
9. **Visually verify the rendered artifact — mandatory.** Screenshot with headless Chrome at a desktop width and a phone-class `390×844` viewport, in light and dark; for a report, also render print/PDF. Look for label collisions, overflow and clipping, contrast in situ, broken layout, and the 30-second argument test. Grepping HTML for expected strings or tags is **not** visual verification: it proves markup exists, not that a human can read it.
10. Score the figure with the `cas-ui-craft` critique (`references/critique-rubric.md`: distinctiveness, fit to argument, hierarchy, craft, accessibility) and append the table to the brief. A public-surface figure ships at 4 or above on the first three and with no mechanical defect (clipped text, a contrast pair under 4.5:1 in either scheme, overlap, phone-width overflow scores 0); `node scripts/visual-qa.mjs <artifact>` PASS is the receipt where the project has it.

## Cassy output contexts

For durable Cassy reports, use static inline SVG and CSS inside one self-contained HTML file: no charting library, CDN, build step, or external asset. This deliberately inverts the bundled skill's interaction-first default: hover is an optional enhancement (CSS-only tooltips are fine), static legibility leads because a figure must survive GitHub embeds, PDF, and print. Use real `<table>` markup for the evidence twin, explicit provenance beneath the figure, and `@media print` rules that retain title, annotation, legend, and table without clipping.

This also inverts two dashboard defaults: use the bundled validator plus the design tokens' roles rather than a full reference theme, and document filters when useful without making them a default for committed evidence artifacts.

GitHub issues and PRs need a compact static SVG or table with the same claim-title and provenance. Terminal-adjacent Markdown should use a small aligned table or Unicode sparkline; do not simulate a dense dashboard in text.

## Guardrails

- No dual axis, decorative rainbow, color-only meaning, tooltip-only value, or number on every point.
- No extra colors to solve too many series: aggregate, facet, small-multiple, or table instead.
- No gradients, glows, drop shadows, or animated counters; a figure earns attention with its claim, not its finish.
- Texture only for print, forced-colors, or an explicit accessibility option, never as decoration.
- Large standalone numbers use proportional figures; aligned table columns and axis ticks use tabular figures.

Read [the design review](references/design-review.md) for the preserve/missing/misfit rationale, [the quality checklist](references/quality-checklist.md) before shipping, and [the worked example](examples/send-backs-dot-strip.html) with its [sidecar](examples/send-backs-dot-strip.why.md) for a self-contained SVG figure whose form was chosen for a reason.
