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

The thread model selects the newest snapshot per session/agent/turn and places it once in the latest work block for that turn. Work rows render the card with the existing unified diff viewer. WorkPicture owns device-local disclosure state and lazy source detail requests. Unknown, loading, offline, missing and failed evidence remain explicit. An unsent or interrupted request waits for the next connection; reconnect makes it eligible again. Closing a disclosure removes its saved entry.

The row keeps its flexible path and trailing operation in separate spans. An optional trailing `+N/−M` readout uses the existing mono size and `--state-ok`/`--state-crit` tokens. The signs and a spoken full-text description carry meaning without color. Known zeroes remain visible; absent counts create no node. The readout wraps locally on narrow screens without changing count-free rows or requesting source details.

## Critique

Scored by the factory worker on 2026-10-30 against production source c44f054054b16dd57d0f68a274c340a92841f970. Chromium on macOS, 1280×800 and 390×844 viewport emulation, light/dark; resting and expanded card states.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Distinctiveness | 4 | The existing Glass disclosure and mono paths now carry a compact signed edit-size readout in the same row. |
| Fit | 4 | Known counts are readable before opening source evidence; absent counts leave the existing row unchanged. |
| Hierarchy | 4 | File paths lead; the trailing operation and quieter mono counts support the scan without competing with replies. |
| Craft | 4 | Strict visual-qa.mjs PASS: eight renders, zero findings and zero allowlisted findings. Largest valid counts wrap to a trailing line while the filename stays readable at 390. |
| Accessibility | 4 | Full spoken latest-edit descriptions, explicit plus/minus signs and known zeroes, computed accessible names and ARIA, both schemes, print checks and three media modes pass. Human screen-reader use was not exercised. |

Four informational contrast checks concern existing gradient buttons (new-session/send), outside the count readout; count text itself has no contrast finding. The JavaScript-required declaration and noscript explanation are unchanged. No visual allowlist rule was added.
