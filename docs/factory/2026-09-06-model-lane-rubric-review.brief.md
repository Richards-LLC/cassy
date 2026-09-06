# Brief: 2026-09-06-model-lane-rubric-review.html

Source: `2026-09-06-model-lane-rubric-review.md`. Type: decision brief. Audience: operator.
Design language: Petrastella tokens (`design-spec/references/design-tokens.json`); `docs/factory`
has no `DESIGN.md` of its own. Previous render preserved as
`2026-09-06-model-lane-rubric-review.before.html`.

## Single idea

Heavy goes to Astra at high, placed by hand; every other lane keeps its model, and no lane runs at
medium — because the host's own cost and send-back numbers say so, not the vendors' reputations.

## Hero form

A dot plot in two dimensions: cost per delivery at list (log axis) against send-back rate, one dot
per model × effort the host has actually run. The geometry is the claim: the four placements sit
along the floor of the plot, one per price tier (Luna $0.49, Haiku $1.46, Astra $12.54, Fable), and
the only two medium runs on record — Astra/medium at 100% and Fable/medium at 133% — are the only
dots above the band. The single indigo mark is the hand-placed Astra/high, because it is the one
placement the reader could disagree with; everything else is quiet evidence.

## Emotional register

Measured, candid, unhurried — sandstone hero, serif verdict, grey evidence dots on warm paper, one
indigo mark, no status colour anywhere above the fold, and every small sample says its n out loud.

## Distinctive move

Sample size is drawn, not footnoted: every measurement with fewer than five deliveries is a hollow
mark with its n printed beside it, in the hero and in every figure below, so a two-delivery Astra
sample can never look like a rate next to Luna's 212.

## Deliberately omitted

The five-card KPI row and the seven-card failures grid of the previous render (numbers in boxes
show no argument); status colour on any model (good/warning/danger mark only the stall event on the
timeline); a single ranking of models (the columns disagree — that is the finding, so the audit
table keeps all five measures side by side); a table as the first element; a table of contents;
the dark gradient hero.

## Critique

Scored by vivid-pelican-81 on 2026-09-06 after rendering at 1280×800 and 390×844 in light and dark,
print preview, and a JavaScript-disabled reload; `node scripts/visual-qa.mjs --strict` PASS (receipt:
`2026-09-06-model-lane-rubric-review.visual-qa.md`; screenshots and JSON under
`~/.cas/artifacts/cas-5d3c/visual-qa/`). Second reader: the supervisor's independent render.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | serif verdict on sandstone with one indigo phrase; every sample under five drawn hollow with its n in the hero and all three figures below; the rubric as a route map crossing a dashed harness line — not 5 because the hollow-with-n move is quiet enough to register only on the second figure |
| Fit to argument | 5 | the hero's shape is the sentence: seven placements inside the 0–16% band, the two medium runs alone at 100% and 133%, the one hollow indigo dot at $12.54 · n=2 is the hand placement |
| Hierarchy | 4 | eyebrow → verdict → figure → 3px rule → provenance, nothing above the verdict; at 390×844 the figure's last row ends at 823px, inside the fold with no room to spare |
| Craft | 4 | visual-qa.mjs --strict PASS (light+dark, 1280+390, 0 findings, 0 informational, 0 allowlisted); tokens only, tabular mono, ruled ledgers with the decisive row banded; seam: the 11–12-column B, B.2 and C ledgers scroll inside their column even at 1280 |
| Accessibility | 5 | every text pair is a sanctioned token pair (minimum 5.22:1, ink-muted on surface-hero, light); no script, so JS-off text is byte-identical; print keeps every figure and expands every details; every figure is a semantic list or a real table with caption and scope plus a table twin; status colour only on timeline events, each with a text tag |

Floor (distinctiveness, fit, hierarchy ≥ 4; no 0): holds.
