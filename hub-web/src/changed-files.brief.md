# Brief: per-turn changed files

## Single idea

The operator can scan each file and its reported latest edit size before deciding which source evidence to open.

## Hero form

Callout rail: one compact disclosure beneath the turn's work summary, followed by a ruled file list only when requested.

## Emotional register

Quiet, inspectable — Glass raised surface, muted provenance, mono paths and counts, native keyboard controls, and existing green/red tokens for addition/removal signs.

## Distinctive move

The file path is the disclosure: opening it reveals the recorded unified diff or the original edit evidence in place, with focus and expansion surviving updates.

## Deliberately omitted

No invented line counts or Git verdict: show optional reported counts only, including known zeroes, and render no counts or placeholder when absent. Counts describe the latest successful edit, never cumulative or net Git totals. No patch wall, split view or review comments.

## Component and state boundary

The thread model selects the newest snapshot per session/agent/turn and places it once in the latest work block for that turn. Work rows render the card with the existing unified diff viewer. WorkPicture owns device-local disclosure state and lazy source detail requests. Unknown, loading, missing and failed evidence remain explicit.

The row keeps its flexible path and trailing operation in separate spans. An optional trailing `+N/−M` readout uses the existing mono size and `--state-ok`/`--state-crit` tokens. The signs and a spoken full-text description carry meaning without color. Known zeroes remain visible; absent counts create no node. The readout wraps locally on narrow screens without changing count-free rows or requesting source details.

## Critique

Scored 2026-10-30 against the production build, with matched 390px and 1440px light/dark captures.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | One Glass disclosure keeps provenance beside mono file paths, within the existing conversation rhythm. |
| Fit | 4 | Scan targets first, then open only the selected recorded diff or exact source evidence. |
| Hierarchy | 4 | Quiet work summary, file-count disclosure, paths, then evidence; no invented metrics compete with the replies. |
| Craft | 4 | Shared strict visual inspector PASS in all four responsive cells; long paths wrap and diff wells scroll locally. |
| Accessibility | 5 | Keyboard focus, reload persistence, named controls, print, forced colors, reduced motion and increased contrast passed. |
