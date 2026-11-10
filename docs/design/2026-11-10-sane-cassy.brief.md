# Brief: 2026-11-10-sane-cassy.html

Audience: practitioner

Report type: decision brief (recommend one option), for the operator, who reads code and wants file-level evidence.

## Single idea

Each of five releases removes more Cassy checks than it adds, taking 388 to 274, and the share that re-reads moving state or parses shell text falls with them.

## Hero form

Sorted bars as a step-down over releases, on a zero baseline, with the moving-state share hatched inside each bar. The descending staircase is the claim: every step goes down, and the reader sees both the total and the hatched part shrink without subtracting anything. The decrement is printed under each release, so the "net reduction per step" acceptance is visible as a column of minus signs.

## Emotional register

Plain and accountable: serif verdict that states the number, quiet grey bars, one violet mark on the end state, no status colour above the fold. It reads like an engineering ledger, not a pitch.

## Distinctive move

The hatching: the same bar carries both the count and the part of it that polices moving state, so "fewer gates" and "saner gates" are one figure. It echoes below in the migration table's last column.

## Deliberately omitted

- No KPI card row of 127 / 46 / 69 / 105 / 41: five boxes would summarise the inventory, not the argument.
- No architecture box diagram above the fold: the reader needs to approve the outcome first; the fact model is a table in the architecture section.
- The full 388-row inventory and the 21-incident ledger are in collapsed details at the end, not in the body.

## Critique

Scored with the cas-ui-craft rubric after render at 1280 and 390 px, light and dark, print preview and JavaScript disabled.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The hatched moving-state share inside each step-down bar is specific to this argument; house tokens, serif verdict and the single violet end bar make it recognisably Cassy. |
| Fit to argument | 5 | The staircase shape is the claim (every release goes down); decrements and the hatched share are read straight off the figure with no prose needed. |
| Hierarchy | 4 | Verdict (23 words), then the figure, then a three-sentence lede; the decision table is the first section. Case detail sits in collapsed details, so the page is about 8,550 px at 1280 px with them closed (estimate 7,851 against the 8,000 budget). |
| Craft | 4 | `visual-qa.mjs --strict` PASS, 0 findings, light and dark at 1280×800 and 390×800, and again at 390×844 (receipt: 2026-11-10-sane-cassy.visual-qa.md). Tabular mono numbers, ruled tables; wide appendix tables scroll inside their container. The hero's axis text is small at 390 px; its numbers are repeated in the chart-data table. |
| Accessibility | 4 | SVG has role, title and description, and its numbers are in a table; hatching plus a labelled key, not colour alone. Details ship `open` in markup, so JavaScript-off shows everything (68,973 px), and the print listener reopens them (45-page PDF). |
