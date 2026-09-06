# Critique rubric

Score every human-facing artifact before it merges. Five dimensions, each 1–5. A public surface —
one an operator, client, or reader outside the session will see — merges only when
**distinctiveness, fit, and hierarchy are each ≥ 4**. Craft and accessibility below 4 are
defects to fix in the same change, not floor failures.

## Dimensions and anchors

| Dimension | 1 | 3 | 5 |
| --- | --- | --- | --- |
| **Distinctiveness** — would a reader know it is ours, and remember one move? | any template's output; all-sans, card grid, gradient hero | house tokens applied; no move a reader would recall | the brief's distinctive move is visible in the first screen and echoed once below it |
| **Fit to argument** — does the hero form's geometry *equal* the single idea? | the hero shows numbers or a table, not a claim | a figure supports the claim but the reader must read prose to see why | the figure's shape is the claim; the prose only names it |
| **Hierarchy** — is the one thing the reader must take away the most prominent thing, and is there exactly one? | several equal-weight blocks compete above the fold | the verdict leads but a competing element (nav, cards, second figure) shares its weight | verdict sentence, decisive figure, rule, then everything else at a clear step down |
| **Craft** — type, rhythm, alignment, rule weights, number formatting | mixed families, inconsistent units, orphaned labels, boxes inside boxes | tokens used correctly with visible seams (uneven gutters, a wrapped header) | nothing to move; every rule, gap, and figure sits where the scale says |
| **Accessibility** — contrast, semantics, keyboard, print, reduced motion | fails a contrast pair or loses content without JS | passes checks with one caveat noted in the brief | passes light, dark, print, JS-off, keyboard, and 390px with no caveat |

## Procedure

1. Render, then open the artifact at 1280×800, 390×844, print preview, and with JS disabled.
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
| Craft | 4 | one wrapped column header at 820px |
| Accessibility | 5 | 6.4:1 minimum; JS-off and print lose nothing |
Scored by <who> on <date>; floor holds.
```

## Common failures and the dimension they hit

- Hero is a KPI card row → fit 1–2, hierarchy 2.
- Dark gradient, rounded cards, all-sans → distinctiveness 1–2.
- Two figures above the fold → hierarchy 2.
- Chart with a legend instead of end labels → craft 3.
- Status colour used as a series colour → accessibility 3, craft 3.
- A table as the first element → fit 2.
