# Project hold

| Field | Contract |
| --- | --- |
| First two lines | State whether the project is held, then give the operator's lift command or confirm normal dispatch and merge admission. |
| Scannable | Reason, operator and recorded time appear as separate labeled rows. |
| Readable | The operator's reason explains why the project stopped; status exposes the complete audit with `--json`. |
| Machine output | `cas --json factory hold status` returns one document with `active` (reason, operator, set_at) and `history` (action, reason, operator, at). |
| Omitted | Worker and delivery inventories stay off the hold screen; normal task queries resume when the operator lifts it. |

Text uses plain ASCII and no ANSI styling; long human values use a bounded preview with `--json` as the full read.

## Critique

Both states were captured from candidate `fd41bad69eb990324f9ef2258eabcf621c05ace3`
in a disposable project. `project-hold-on` and `project-hold-off` each recorded
`terminal-qa: PASS`: 12 runs, 0 failures and 0 warnings. Captures and full receipts
are task artifacts `terminal-on/report.json` and `terminal-off/report.json`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | Hold state and cause lead; the operator's lift command follows the audit rows. |
| Fit | 5 | Held output measures 41 cells at both 80 and 120 columns. |
| Craft | 4 | One labeled row per fact; ordinary admission returns a two-line off state. |
| Theme safety | 5 | Four palettes, NO_COLOR, pipe and C locale pass without ANSI dependence. |
| Machine contract | 5 | JSON emits active state and full history in one document. |

Scored by the implementation worker on 2026-11-03. The positive operator
mutation path is tested with an explicit authority fixture; a real agent CLI
is refused for both set and lift.
