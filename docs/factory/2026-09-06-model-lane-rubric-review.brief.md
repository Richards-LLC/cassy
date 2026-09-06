# Brief: 2026-09-06-model-lane-rubric-review.html

Source: `2026-09-06-model-lane-rubric-review.md`. Type: decision brief. Audience: operator.
Design language: Petrastella tokens (`design-tokens.json`); no project `DESIGN.md` applies to `docs/factory`.

## Single idea
The rubric assigns lanes by reputation, and the one evidence window we have contradicts it lane by lane.

## Hero form
A ledger: one row per lane, *reputation says* beside *data says*, with a verdict stamp in the gutter
between them (CONSISTENT / CONTRADICTED / UNTESTED / UNUSED). A contradiction is spatial — the claim
and the counter-evidence must sit on one line — so the ledger's geometry *is* the idea. The data
column carries a dot strip (one dot per delivery, a ring per send-back) so magnitude and failure rate
are visible before any number is read. The seven rubric failures become marginal notes anchored to
the rows they indict.

## Emotional register
Forensic, unhurried — sandstone hero surface, serif verdict sentence, ruled ledger on warm paper,
indigo reserved for the decisive marks, no status colour above the fold except inside the stamps.

## Distinctive move
The gutter stamps: spaced small-capital serif verdicts on a verdict-soft band, the only display-face
text below the title. The eye reads the five stamps top to bottom before reading a sentence.

## Deliberately omitted
The five-card KPI row of the previous render (it summarised the document and showed no argument);
the gradient hero and the seven-card failures grid; colour as the only carrier of any status — every
stamp is text, every send-back is a ring, every scenario is a fill.

## Evidence and closing figure
Small multiples on one shared 0–20 scale (workers, deliveries, send-backs, merged per lane) so the
standard lane towers over the rest by construction; release latency as two panels on a shared 0–5 h
scale. The model intelligence, cost and efficiency section (cas-3372, cas-de0b) is rendered as ruled
ledgers with a log-scale cost-per-delivery figure (Astra dashed as a sample of two) and four
small-multiple scorecard panels; its two findings are cards, its placement rules a second ladder.
The closing figure is the decision: options A and B drawn as the heavy lane's primary → fallback
edge, B stamped RECOMMENDED with the promotion rule as its annotation.

## Critique
Scored by wise-falcon-12, 2026-09-06, on the four headless renders (1280 and 390, light and dark)
plus print preview and JS-off reload; by-hand mechanical checks pending `scripts/visual-qa.mjs`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | serif stamps in the ledger gutter; marginal-note rail; sandstone hero with a 3px indigo rule |
| Fit to argument | 5 | the hero is the contradiction itself: five rows, five stamps, three of them not CONSISTENT |
| Hierarchy | 4 | stamps → title → ledger rows → notes → evidence → decision; the KPI competition is gone |
| Craft | 4 | no clipped text or crossed marks on any of the four renders; ledger stacks at 390px; print breaks by row |
| Accessibility | 4 | every text pair ≥ 4.5:1 in light and dark (token pairs table); stamps and rings are text/shape; every figure has a table twin and `aria-label` |

Floor (distinctiveness, fit, hierarchy ≥ 4; no 0): met.
