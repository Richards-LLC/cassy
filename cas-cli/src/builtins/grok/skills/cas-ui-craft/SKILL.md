---
name: cas-ui-craft
description: Use when designing or rendering any human-facing surface — an HTML report, dashboard, product or landing page, README hero, slide, or application screen — before the first render, and when critiquing one before it merges to a public surface. Owns the concept brief, the first-three-seconds rule, the form vocabulary, and the scored critique rubric; cas-html-reports keeps the report contract and cas-dataviz the figure contract.
managed_by: cas
---

# UI craft

A surface is finished when a reader gets the argument in three seconds and would recognise the
page as ours. Compliance with a contract is the floor; this skill is the ceiling.

## Steps

1. **Write the concept brief before any markup.** Create `<artifact-basename>.brief.md` beside the
   artifact using [references/concept-brief.md](references/concept-brief.md). Done when all five
   fields hold a specific sentence a stranger could disagree with — a field that would fit any
   page is not filled.
2. **Inherit the design language.** Read the project's `DESIGN.md`; without one, read
   [../design-spec/references/petrastella-design-language.md](../design-spec/references/petrastella-design-language.md)
   and its `design-tokens.json`. Done when the artifact's `:root` declares tokens by the names in
   that source and no rule carries a hex value the tokens do not. A neutral palette is chosen
   only when the brief's *omitted* field names the brand reason.
3. **Compose the first screen around the argument as a figure.** The verdict sentence and the one
   figure that proves it are both fully visible at 1280×800 and at 390×844 with no scroll; nothing
   else sits above them. Done when two headless-Chrome screenshots at those sizes show sentence and
   figure with no KPI-card row, table of contents, or logo band ahead of them.
4. **Choose every section's form from the vocabulary.** Pick from
   [references/form-vocabulary.md](references/form-vocabulary.md) by the reader's task; the
   brief's *hero form* names the first screen's. Done when no two adjacent sections share a form
   (small multiples excepted), and at most one plain card grid appears on the page.
5. **Keep the invariant constraints.** One file, no network, semantic HTML, contrast ≥ 4.5:1
   text and ≥ 3:1 marks in light and dark, keyboard-reachable controls, a text alternative and
   data table per figure, a print stylesheet, `prefers-reduced-motion` respected. For a report,
   `cas-html-reports/references/technical-contract.md` is the full list; apply the same list to
   any other surface. No fixed height on a text-bearing box without a declared overflow
   strategy; a rule that sets a background sets its foreground. Done when a JS-disabled reload
   and a print preview lose nothing and `node scripts/visual-qa.mjs <artifact>` prints PASS
   (light and dark contrast per node, clipping, overlap, 390px overflow) — or, where the project
   lacks the script, the same four checks were made by eye on both schemes and the brief says so.
6. **Critique with the rubric before merge.** Score the artifact 1–5 on each dimension in
   [references/critique-rubric.md](references/critique-rubric.md) and append the scored table to
   the brief under `## Critique`. A public surface (anything an operator, client, or reader outside
   the session will see) merges only with distinctiveness, fit, and hierarchy each ≥ 4 and no
   dimension at 0 (a mechanical defect — clipped text, a contrast pair under 4.5:1 in either
   scheme, overlap, phone-width overflow — is a 0); below the floor, revise the brief's
   *distinctive move* and re-render — do not ship and annotate. Done when the table is in the
   brief and every floor holds.
7. **Commit brief and artifact together**, plus the markdown source for a report. Done when one
   commit contains all of them.

## The first three seconds

The reader looks at the top of the page and, before reading a word of prose, sees a sentence that
states the conclusion and a figure whose shape *is* that conclusion — a slope that falls, a dot
that sits outside the band, a bar that dwarfs its neighbours. A hero that shows four numbers in
boxes shows no argument; a hero that shows a table asks the reader to build one.

## Exemplars

Each carries its concept brief, visible design notes on every decision, and its rubric scores.
Open them in a browser and read the source.

- [references/exemplars/report.html](references/exemplars/report.html) — incident review:
  verdict hero with an annotated timeline as the figure, evidence ledger, small multiples,
  marginal notes.
- [references/exemplars/dashboard.html](references/exemplars/dashboard.html) — weekly delivery
  health: a slope chart as the argument, a stat strip instead of KPI cards, waffle plot, ledger.
- [references/exemplars/product-page.html](references/exemplars/product-page.html) — a launch
  page whose hero is a dot plot of the claim, one ledger column, pull-quote, ruled pricing.
- [references/exemplars/before-after.html](references/exemplars/before-after.html) — the same
  data rendered our old way (cards, table, dark gradient) and this way, each scored on the rubric.

## Scope boundaries

`cas-html-reports` owns the report type × audience matrix, provenance, and the markdown-first
workflow; `cas-dataviz` owns chart selection, number formatting, and palette validation;
`design-spec` owns `DESIGN.md`. This skill sits between the markdown and the render: it decides
what the page is *for* and whether the result is good enough to carry the name.
