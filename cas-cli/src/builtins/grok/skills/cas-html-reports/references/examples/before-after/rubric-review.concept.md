# Concept brief — Model lane rubric review (2026-09-06)

Source: `2026-09-06-model-lane-rubric-review.md`. Render: `2026-09-06-model-lane-rubric-review.html`.
Type: decision brief. Audience: operator (executive depth, practitioner vocabulary allowed).
Design language: Petrastella default (no project `DESIGN.md` applies to `docs/factory`).

## Single idea

The rubric assigns lanes by reputation; the one evidence window we have contradicts it lane by lane.

## Hero form and why

A **ledger**: one row per lane, two aligned columns — *reputation says* (what the registry's
assignment implies) and *data says* (what the window measured) — with a verdict stamp in the gutter
between them (CONSISTENT, CONTRADICTED, UNTESTED, UNUSED). The argument is a contradiction, and a
contradiction is spatial: the reader must see the claim and the counter-evidence on the same line.
The data column carries a dot strip (one dot per delivery, ringed when sent back) so the magnitude
and the failure rate are visible without reading a number. The seven rubric failures become
**marginal notes** anchored to the rows they indict, instead of a card grid detached from the evidence.

## Emotional register

Forensic and unhurried. A ledger, not a dashboard: warm paper, ink, one accent for the verdict.
No gradients, no glow, no KPI boxes competing for the first glance.

## One distinctive move

The gutter stamps, set in the display serif in spaced small capitals, are the only display-face text
below the title. The eye reads the five stamps top to bottom before anything else and has the verdict
before reading a sentence.

## Deliberately omitted

- The five-card KPI row of the previous render: it summarized the document, it did not show the argument.
- The gradient hero and the seven-card "failures" grid: decoration and detachment respectively.
- Color as the carrier of any status: every stamp is text, every send-back is a ring, every scenario
  is a fill.

## Evidence and closing figure

Evidence as **small multiples**: one panel per lane on a shared 0–20 scale (workers, deliveries,
send-backs, merged) so the standard lane towers over the rest by construction; release latency as
two panels on a shared 0–5 h scale. The **closing figure** is the decision itself: options A and B
drawn as the heavy lane's primary → fallback edge under each option, B stamped RECOMMENDED with the
promotion rule as its annotation.

## Critique (cas-ui-craft rubric, 1–5)

| Dimension | Score | Note |
| --- | --- | --- |
| Distinctiveness | 4 | Ledger with stamps and marginal notes; the serif stamps are the signature |
| Fit to argument | 5 | The hero is the contradiction; nothing else is above the fold |
| Hierarchy | 4 | Stamps → title → ledger rows → notes → evidence → decision; KPI competition removed |
| Craft | 4 | Verified at 1280×800, 390×844, print; dot strips share one unit; all figures have table twins |
| Accessibility | 4 | Contrast ≥ 4.5:1 light and dark; stamps and rings are text/shape, not color; real tables |

Floor (4 on the first three): met.
