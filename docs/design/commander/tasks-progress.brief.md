# Brief: Tasks & progress

## Single idea
The selected supervisor's release is moving toward a finish, and the next decision is visible without reading the whole board.

## Hero form
Progress strip: one violet completion bar with merged/total, merge rate and the last merge directly underneath the focused epic title makes remaining work visible as distance.

## Emotional register
Calm, purposeful — existing Glass surfaces, violet progress, quiet ruled task groups and compact permission disclosure.

## Distinctive move
A release completion strip leads a working ledger: Needs you, In progress, Ready; the full board is an explicit All tasks view.

## Deliberately omitted
No disabled-control wall or project-wide task dump in the default view. Unknown producer metrics stay explicitly unavailable instead of estimating merges from closed tasks.

## Implementation contract
- Follow `hub-web/DESIGN.md`, `src/tokens.css` and `src/glass.css`; existing semantic tokens only.
- `progress-model.ts` derives epic selection and task groups from server data. Focused epic wins; otherwise choose the epic with most active children. Scope to reported supervisor epics and roster tasks; unknown membership stays in All tasks when epics exist.
- `progress-view.ts` owns the release strip. `main.ts` owns All tasks interaction and worker/task rendering. Worker cards preserve unknown detail and add reported activity only.
- `context-resize.ts` owns local width, pointer capture, keyboard adjustments and localStorage persistence. Desktop only, minimum 240px, maximum 480px subject to preserving the conversation; 390px remains a full-screen sheet.
- Permissions use one disclosure line and a Paired machines link; available controls retain their existing handlers.
- Scrollbars use ink-muted and glass tokens, with system colors in forced colors. No motion is added.
- Test focused epic metrics, scoped grouping, permission disclosure, keyboard/pointer bounds and storage recovery through existing modules and the conversation journey. Capture baseline and final at 1280, 1600 and 390px in both themes.

## Critique
| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The house serif release title and violet completion strip lead a quiet working ledger on Glass. |
| Fit | 4 | The filled portion of one bar directly expresses merged work; unknown metrics explicitly remain unavailable. |
| Hierarchy | 4 | Release title, completion, rate and time step down in order; worker cards and task groups use smaller labels. |
| Craft | 4 | Strict visual QA PASS at 1280, 1600 and 390px in light and dark, zero findings; existing project allowlist only. |
| Accessibility | 4 | Named native progress element and permission disclosure; pointer/keyboard resize; system forced colors; no added motion. Reviewed JavaScript application declaration remains in force. |

Scored by the implementing worker, 2026-10-28. Public-surface floors hold.

## Handoff and constraints
- Server data stays authoritative. Missing epic statistics, liveness or timestamps never imply healthy/idle/zero; their unavailable states remain visible. The separate producer task owns the real epic_status readback and the merge collector.
- All tasks is local to each machine/session; width is localStorage on the browser device and is clamped at the current viewport without overwriting its wider preference.
- Release/grouping, permission disclosure and resize tests fail on the base implementation. HUB-J7 exercises filtering, resize by keyboard and pointer, persistence storage, phone width and media modes against the production bundle. HUB-J17 and HUB-J22 retain their permission and pairing paths with the new disclosure.
- Screenshots and static DOM captures come from the actual production bundle with the hub protocol double. Baseline used the legacy data fields; treatment additionally carries the agreed optional progress fields. This is disclosed rather than presenting the data fixtures as live telemetry.
- Strict QA checks all six rendered states, print and the reviewed JavaScript declaration. Forced colors, reduced motion and increased contrast are captured by HUB-J7. No image assets or animation were added.
- Vite reports initial JavaScript 190.71 KB gzip and total CSS 36.23 KB gzip. The JavaScript budget holds. The existing stylesheet already exceeds the generic 30 KB budget; reducing the shared stylesheet is outside this panel change and remains a frontend maintenance follow-up.
- Durable evidence: task artifact directory `cas-5a42/`, including `before/`, `evaluation/`, `qa/visual-qa/`, `LEDGER.md` and `qa/bundle.json`.

