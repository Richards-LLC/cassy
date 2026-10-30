# Brief: per-turn changed files

## Single idea
The operator can scan a turn's tool-reported edits before deciding which source evidence to open.

## Hero form
Callout rail: one compact disclosure beneath the turn's work summary, followed by a ruled file list only when requested.

## Emotional register
Quiet, inspectable — Glass raised surface, muted provenance, mono paths and native keyboard controls.

## Distinctive move
The file path is the disclosure: opening it reveals the recorded unified diff or the original edit evidence in place, with focus and expansion surviving updates.

## Deliberately omitted
No invented line counts or Git verdict: v2 reports successful tool calls and explicitly leaves shell side effects unknown. No patch wall, split view or review comments.

## Component and state boundary
The thread model selects the newest snapshot per session/agent/turn and places it once in the latest work block for that turn. Work rows render the card with the existing unified diff viewer. WorkPicture owns device-local disclosure state and lazy source detail requests. Unknown, loading, missing and failed evidence remain explicit.

The row keeps its flexible path and trailing operation in separate spans. A follow-up may add an optional counts span beside the operation when `line_counts: {added, removed}` lands; those counts describe the latest successful edit, never a cumulative or Git total. This card does not render them.

## Critique

Scored 2026-10-30 against the production build, with matched 390px and 1440px light/dark captures.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | One Glass disclosure keeps provenance beside mono file paths, within the existing conversation rhythm. |
| Fit | 4 | Scan targets first, then open only the selected recorded diff or exact source evidence. |
| Hierarchy | 4 | Quiet work summary, file-count disclosure, paths, then evidence; no invented metrics compete with the replies. |
| Craft | 4 | Shared strict visual inspector PASS in all four responsive cells; long paths wrap and diff wells scroll locally. |
| Accessibility | 5 | Keyboard focus, reload persistence, named controls, print, forced colors, reduced motion and increased contrast passed. |
