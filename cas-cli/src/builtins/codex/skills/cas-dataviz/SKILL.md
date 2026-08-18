---
name: cas-dataviz
description: Use when work involves a chart, graph, plot, dashboard, visualization, heatmap, sparkline, palette, or “visualize data”; when a report is about to include quantitative comparisons; or when a document/report is becoming text-dense.
managed_by: cas
---

# Data visualization that makes a point

Claude sessions may also carry a bundled `dataviz` skill; this Cassy skill is the canonical cross-harness companion. It keeps the useful color discipline, while fitting Cassy’s durable, self-contained HTML report surfaces.

## Start with the message, not the chart

Write one sentence that says what the reader should learn and make it the chart title. “Failures by class” only names an axis; “Merge commits were the largest class today, twice the next largest” makes a claim the graphic must prove. If no useful sentence emerges, use a table or continue investigating.

Put the evidence *on the chart*: annotate the decisive point, bar, interval, or threshold with a brief callout and a connector when necessary. Do not make a reader infer the conclusion from a paragraph beside an unmarked plot.

## Give dense documents visual rhythm

When quantitative or enumerable prose becomes a wall of text, convert the part readers must compare into a stat tile, table, chart, or timeline. Give long reports a hero number or key-finding tile near the top, pace sections with figures/tables, and use callout blocks for load-bearing facts. **Acceptance test:** a reader can get the argument from the visuals and claim-titles alone in 30 seconds.

## Choose the least-ink form

| Reader’s task | Prefer | Avoid |
| --- | --- | --- |
| One current value | stat tile or hero number | one-bar chart |
| Compare magnitudes | sorted bar chart | pie for close values |
| Show change over time | line; area only for one series | dual axes |
| Compare before/after | dumbbell or paired bars | disconnected columns |
| Show composition | stacked bar, usually horizontal | more than six pie slices |
| Show a matrix | heatmap plus values/table | rainbow scale |
| Preserve exact evidence or many classes | table, optionally paired with a chart | a chart carrying more than ~7 meanings |

Use small multiples when the comparison is among several similarly shaped series; share scales and alignment so differences, not chart furniture, carry the work. Never use two y-axes: split, facet, or index both series to a common baseline.

## Procedure

1. State the message as a claim-title, name the population, time window, unit, and source.
2. Pick the form from the reader’s task. Prefer a table when exact lookup, many categories, or auditability is primary.
3. Format numbers before drawing: use compact figures only where scanning benefits (`12.4K`, `$4.2M`); keep the unit in the title/subtitle or axis, choose a consistent precision, use thousands separators, and do not mix percent, fraction, and percentage-point changes.
4. Show uncertainty when the data has it: intervals/bands for estimates, n for samples, missing-data marks, and a plainly labelled baseline/forecast. Do not draw false precision from rounded or partial data.
5. Draw quiet structure: direct labels only for the endpoint, extreme, or annotated insight; hairline solid axes/grid; thin marks; adequate white space. A legend supports two or more series, but is not a substitute for annotation.
6. Assign color last. Use one hue, light-to-dark, for magnitude; fixed categorical colors for identity; a warm/cool pair with neutral midpoint for polarity; and reserve status colors for status. Color follows an entity, never its rank after filtering.
7. Validate any categorical palette with `node scripts/validate_palette.js "#hex,#hex" --surface "#ffffff"`; do not eyeball contrast or separability. The validator is bundled in every harness mirror.
8. Add a text alternative and an adjacent data table. In a report, chart sections follow `cas-dataviz`; the report’s own contract remains in `cas-html-reports`.
9. **Visually verify the rendered artifact — mandatory.** Use headless Chrome to screenshot at a desktop width and a phone-class `390×844` viewport; for a report, also render print/PDF. Look at those renders for label collisions, overflow/clipping, contrast in situ, broken layout, and the 30-second visual-argument test. Grepping HTML for expected strings or tags is **not** visual verification and never satisfies this check: it proves markup exists, not that a human can read it. Follow the H7 acceptance-report precedent: headless Chrome at `390×844` plus PDF render.

## Cassy output contexts

For durable Cassy reports, use static inline SVG and CSS inside one self-contained HTML file: no charting library, CDN, build step, or external asset. This deliberately inverts the bundled skill’s interaction-first default: hover is an optional enhancement (CSS-only tooltips are fine), while static legibility leads because a chart must survive GitHub embeds, PDF, and print. Use real `<table>` markup for the evidence twin, explicit provenance beneath the figure, and `@media print` rules that retain title, annotation, legend, and table without clipping.

This also deliberately inverts two dashboard defaults: use the bundled validator plus a minimal local palette rather than a full reference theme, and document filters when useful but do not make them a default for committed evidence artifacts.

GitHub issues and PRs need a compact static SVG or table with the same claim-title and provenance. Terminal-adjacent Markdown should usually use a small aligned table or Unicode sparkline; do not simulate a dense dashboard in text.

## Guardrails

- Do not use a dual axis, decorative rainbow, color-only meaning, a tooltip-only value, or a number on every point.
- Do not use more colors to solve too many series: aggregate, facet, small-multiple, or table instead.
- Use texture only for print, forced-colors, or an explicit accessibility option, never as decoration.
- Large standalone numbers use proportional figures; aligned table columns and axis ticks may use tabular figures.

Read [the design review](references/design-review.md) for the preserve/missing/misfit rationale, [the quality checklist](references/quality-checklist.md) before shipping, and [the worked example](examples/2026-08-11-commit-classes.html) for a self-contained SVG report figure.
