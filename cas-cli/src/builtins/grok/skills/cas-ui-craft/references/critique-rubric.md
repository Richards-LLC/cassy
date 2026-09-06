# Critique rubric

Score every human-facing artifact before it merges. Five dimensions, each 1–5, with **0** reserved
for a mechanical defect. A public surface — one an operator, client, or reader outside the
session will see — merges only when **distinctiveness, fit, and hierarchy are each ≥ 4** and
**no dimension is 0**. Craft and accessibility between 1 and 3 are defects to fix in the same
change, not floor failures.

## Mechanical defects score 0

Any one of these sets craft (or accessibility, where named) to 0 and blocks merge regardless of
the other scores. They are the defects that recur on our surfaces; taste does not excuse them.

- A text-on-surface pair below 4.5:1 in **either** scheme (light or dark), including white-on-white
  from a surface that set its background but not its foreground — accessibility 0.
- Text clipped by `overflow: hidden`, a fixed height, or a container edge — craft 0.
- A border, rule, or neighbouring element crossing a glyph or a chart mark (overlap) — craft 0.
- A text node that does not wrap or scroll inside its container at 390px, or any page-level
  horizontal scroll — craft 0.
- Content lost with JavaScript disabled or in print — accessibility 0.

The receipt for this class is the visual-QA run: `node scripts/visual-qa.mjs <artifact>` renders
the page headless in light and dark, checks every text node's contrast and every box for
clipping, overlap, and phone-width overflow, and prints PASS or the failing nodes. Paste the
PASS line into the critique table's craft evidence. Until the script exists in the project,
perform the same checks by hand on the four renders in step 1 and say so in the evidence.

For a mergeable public surface, run `node scripts/visual-qa.mjs <artifact> --strict` and require a
committed `docs/factory/data/visual-qa/visual-qa.md` PASS plus matching JSON/screenshots; review
every allowlist entry for its finding type, selector, and specific reason.

## Dimensions and anchors

| Dimension | 1 | 3 | 5 |
| --- | --- | --- | --- |
| **Distinctiveness** — would a reader know it is ours, and remember one move? | any template's output; all-sans, card grid, gradient hero | house tokens applied; no move a reader would recall | the brief's distinctive move is visible in the first screen and echoed once below it |
| **Fit to argument** — does the hero form's geometry *equal* the single idea? | the hero shows numbers or a table, not a claim | a figure supports the claim but the reader must read prose to see why | the figure's shape is the claim; the prose only names it |
| **Hierarchy** — is the one thing the reader must take away the most prominent thing, and is there exactly one? | several equal-weight blocks compete above the fold | the verdict leads but a competing element (nav, cards, second figure) shares its weight | verdict sentence, decisive figure, rule, then everything else at a clear step down |
| **Craft** — type, rhythm, alignment, rule weights, number formatting; 0 on any mechanical defect | mixed families, inconsistent units, orphaned labels, boxes inside boxes | tokens used correctly with visible seams (uneven gutters, a wrapped header) | visual-QA PASS and nothing to move; every rule, gap, and figure sits where the scale says |
| **Accessibility** — contrast, semantics, keyboard, print, reduced motion | fails a contrast pair or loses content without JS | passes checks with one caveat noted in the brief | passes light, dark, print, JS-off, keyboard, and 390px with no caveat |

## Procedure

1. Render, then open the artifact at 1280×800, 390×844, print preview, and with JS disabled, in
   light and in dark; run `node scripts/visual-qa.mjs <artifact>` where the project has it.
2. Score each dimension against the anchors; a score needs one sentence of evidence naming what
   on the page earned it. "Looks good" is not evidence.
3. Append the table to the brief under `## Critique`, with the scorer and date.
4. If any floor fails: rewrite the brief's *distinctive move* or *hero form*, re-render, re-score.
   Two failed rounds means the single idea is wrong; go back to the markdown.
5. When a second reader is available (a fresh subagent, or a taste-lane reviewer), have them score
   blind and record both columns. Disagreements of two or more points are resolved by re-rendering,
   not by averaging.

## Score table

```markdown
| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | serif verdict over sandstone; timeline runs the margin column |
| Fit to argument | 5 | the slope from 31% to 9% is the sentence |
| Hierarchy | 4 | ledger competes slightly with the figure at 390px |
| Craft | 4 | visual-qa.mjs PASS (light+dark, 0 clipped, 0 overlaps); one wrapped column header at 820px |
| Accessibility | 5 | 6.4:1 minimum; JS-off and print lose nothing |
Scored by <who> on <date>; floor holds.
```

## Common failures and the dimension they hit

- Hero is a KPI card row → fit 1–2, hierarchy 2.
- Dark gradient, rounded cards, all-sans → distinctiveness 1–2.
- Two figures above the fold → hierarchy 2.
- Chart with a legend instead of end labels → craft 3.
- Clipped caption, border through a label, text unreadable in dark mode → craft or accessibility 0; not mergeable.
- Status colour used as a series colour → accessibility 3, craft 3.
- A table as the first element → fit 2.
