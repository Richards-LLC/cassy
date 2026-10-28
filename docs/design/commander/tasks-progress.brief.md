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
Pending rendered review.
