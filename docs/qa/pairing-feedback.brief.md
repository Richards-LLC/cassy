# Brief: pairing feedback and browser identity (cas-2e77)

## Single idea

Pairing must show what stopped it, and a revocation decision must name the browser the operator recognizes.

## Hero form

Keep the existing pairing form and device list: focus and scroll the failed attempt's advice into the form's visible area; lead each installation row with its browser name, ownership and readable activity.

## Emotional register

Calm and specific: retain Commander's existing dialog, ink and focus tokens, and give the failed attempt a reachable next action.

## Distinctive move

The same hierarchy serves both decisions: human outcome first, exact engineering evidence in a closed Technical details disclosure.

## Deliberately omitted

No extra confirmation, credential reset or new permission: pairing retry/cancel and exact-device revocation retain their existing protocol semantics.

## Critique

Scored by rapid-lynx-72 on 2026-10-06 against native production-dist captures at a437b0fdf (product identical to 9adc93435). The supervisor independently confirmed 14/14 at c2310e249 with rebuilt dist; the worker capture pass retained 13 passing parts and one failed QA export, then the corrected enrollment export passed once.

| Dimension | Score | Rendered evidence |
| --- | --- | --- |
| Distinctiveness | 4 | Cassy wordmark and existing ink/focus tokens frame the same human-first hierarchy in both dialogs; engineering identifiers wait behind Technical details. |
| Fit | 4 | The failed attempt's focused advice and reachable Pair directly answer what to do next; browser name, ownership and access answer which device to revoke. |
| Hierarchy | 4 | The advice has a clear focus outline above actions; Work laptop · This browser leads the installation record before activity and the closed disclosure. |
| Craft | 4 | Strict actual-DOM exports PASS: eight renders at 1280×800 and 390×844, light/dark, zero unsuppressed findings. Four inactive connection-toast findings are explicitly reviewed below. |
| Accessibility | 4 | Native focus/alert/viewport geometry, keyboard disclosure and opener restoration pass; forced-colors, reduced-motion and more-contrast captures retain human account text. Static exports pass print and JS-off content checks; the interactive app still requires JavaScript. |

Strict evidence: task artifact `cas-2e77/strict-final-9adc93435/visual-qa.json`, individual standard tool reports and stdout. The exception is limited to `invisible-text` on `#toast:not(.visible)`: the existing connection toast removes `.visible` after 3200ms and intentionally retains its previous string at opacity zero outside the modal. No visible toast, pairing advice, installation row, contrast, clipping, overlap or overflow finding is exempted. The initial strict run's four findings remain recorded.

Independent QA round 1 approved the flow and identified three follow-up refinements: inset focus-ring clearance, resetting the failure alert during a retry, and clamping machine timestamps ahead of this browser's clock. The follow-up adds an inline text gutter, restores a polite status before progress text, and labels future activity “Just now” while keeping exact local and UTC times available. Past activity still reads “4 hours ago”/“2 hours ago”. Final captures and critique for this delta are recorded in the task artifact ledger after the rebuilt bundle is exercised.

All five scores meet the floor. The phone's scrolled introductory prose is available in the form; the complete error and both actions remain in view. This review covers the changed dialogs, not a new 19-journey assembly pass.

The supervisor also folded cas-c15d into this delivery: long browser headings wrap, blank-after-trim browser names are refused, revoke consent uses the trimmed displayed name, an in-flight exchange keeps focus on Cancel, and retry advice uses device-neutral “press Pair”. The final bound pass retains assertions for these paths.
