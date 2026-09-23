# Brief: 2026-09-23-model-lane-rubric-refresh.html

Source: `2026-09-23-model-lane-rubric-refresh.md`. Type: decision brief. Audience: operator.

## Single idea

Keep the 2026-09-06 lane intent only as a receipt-gated provisional policy: the new window has
many Astra/high deliveries but no Sol/high control, so it cannot justify a heavy-lane promotion.

## Hero form

Small horizontal dot plot: Luna/xhigh, Astra/high, and Sol/high share one send-back-rate axis; the
highlighted Astra mark sits above Luna while Sol is an outlined zero-sample row. That geometry makes
the decision boundary visible: a larger Astra sample is not the missing paired comparison.

## Emotional register

Measured, candid — warm sandstone for the verdict, quiet evidence grey, one indigo mark for Astra,
and explicit hollow marks for absent controls.

## Distinctive move

The missing Sol/high control is drawn as an outlined row with `n=0` beside the measured Astra and
Luna marks, so the reader cannot mistake volume for a fair comparison.

## Deliberately omitted

No KPI-card row, model leaderboard, or routing diff: cards would flatten the decision, a leaderboard
would imply a controlled experiment that does not exist, and policy edits are outside this brief.

## Critique

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Distinctiveness | 4 | Sandstone verdict hero, serif conclusion, and the outlined `Sol/high · n=0` control row repeat the brief's missing-experiment move; no card grid or gradient. |
| Fit to argument | 5 | The shared send-back axis places Luna at 23.81%, Astra at 29.79%, and the hollow Sol row at zero observations; the figure is the evidence boundary. |
| Hierarchy | 5 | The verdict and figure are first at desktop and phone widths; the indigo rule and marked heavy row are the only competing emphasis. |
| Craft | 4 | `node scripts/visual-qa.mjs --strict` PASS: light/dark, 1280×800 and 390×800; an explicit 390×844 run also PASSed; tables scroll inside their own containers. |
| Accessibility | 5 | Visual QA found 0 contrast, clipping, overlap, overflow, print-loss, or JavaScript-disabled findings; the report is static HTML with semantic tables, SVG title/text alternative, print CSS, and visible link labels. |

Scored by Codex on 2026-09-23; floor holds. Receipts: `~/.cas/artifacts/cas-8505/visual-qa/visual-qa.md` and `~/.cas/artifacts/cas-8505/visual-qa-844/visual-qa.md`.
