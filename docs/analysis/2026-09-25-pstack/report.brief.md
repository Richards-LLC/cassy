# Brief: report.html (pstack vs Cassy, operator report)

Cell: comparison (decision brief), with the operator as audience. Source: `SYNTHESIS.md` beside
this file.

## Single idea

Cassy enforces and pstack judges, so the operator should add pstack's judgment layer on top of
Cassy's gates, starting with the S-effort items that fit the current epic.

## Hero form

A **mirror bar chart** (a dumbbell unfolded around a spine). Each capability is one row. Cassy's
strength extends left of a centre rule and pstack's extends right, on one shared 0–3 scale. Rows
are grouped by which side leads, so the chart's shape is the claim: a heavy left block (Cassy's
mechanism rows) sits above a heavy right block (pstack's judgment rows). The reader sees "two
different halves" before reading a word. The leading side of each row is solid and the trailing
side is an outline, so the lead reads by fill and not only by colour.

## Emotional register

Settled and practical. The choices that earn it: a sandstone hero surface, a serif verdict that
names both sides in eight words, no status colour above the fold, and the adopt ledger ruled, not
boxed.

## Distinctive move

The **one indigo mark** is a bracket along the pstack-led block, labelled "adopt this layer". It
turns the comparison into an instruction. The same bracket label heads the adopt ledger below,
which echoes the move once.

## Deliberately omitted

- No KPI card row (such as "17 adopt / 12 skip"). Counts show no argument.
- No radar chart of principles. The design language bans it, and it hides which side leads.
- No value × effort scatter as the hero. Seventeen labelled points collide at 390 px, and the
  demo needs the ranked list with its mapping, which a ledger shows directly.
- No per-principle table (23 rows). It stays in the P2 lane doc, linked.

## Critique

Scorer: happy-raven-25 (self), 2026-09-25. Renders: 1280 and 390, light and dark. The receipt is `visual-qa.mjs --strict` PASS, 0 findings, run 5 of 5, under
`~/.cas/artifacts/cas-a498/visual-qa/`, with a copy of `visual-qa.md` beside this file as `report.visual-qa.md`. Also checked: print media at 900 px and an A4 PDF (`~/.cas/artifacts/cas-a498/print-run2/`), and a JS-disabled load, where all 11,586 characters of body text are present (the page ships no script).

Revisions from the renders:

- run 1: left score numbers collided with the labels, and the pstack block's spine was offset. The fix gave the numbers their own grid columns and matched the group padding.
- run 2: at 390 px the bracket label overlapped the group label. The fix turns the bracket into a top rule with its label inside.
- run 3: the phone caption was squeezed to one word wide, and file paths broke mid-word. The fix makes the caption a block and adds `<wbr>` after `/` and `_`.
- run 4: phone ledger rows ran one field per line. The fix puts rank, value and effort on one line.

| Dimension | Score | Evidence |
|---|---:|---|
| Distinctiveness | 4 | The mirror chart plus the single indigo "adopt this layer" bracket is visible in the first screen, and the same bracket label heads the ledger. Serif verdict on sandstone. It uses no template card grid. |
| Fit to argument | 5 | The figure's shape is the claim: the Cassy-led block extends left and the pstack-led block extends right. The verdict sentence only names what the geometry already shows. |
| Hierarchy | 4 | Verdict, then mirror chart, then the "adopt this layer" ledger. The data table under the figure is a quiet step down (evidence colour). The skip list and provenance sit at the bottom. |
| Craft | 4 | visual-qa `--strict` PASS in light and dark at 1280 and 390. Mono tabular numbers, one rule weight per role, and an 8 px panel radius only on the figure. |
| Accessibility | 4 | The chart is `role="img"` with a text summary and a real data table. Every table has a caption and scoped headers. Focus ring uses the action token. Print expands everything. The caveat: at 390 px the ledger restacks rows instead of scrolling. |
