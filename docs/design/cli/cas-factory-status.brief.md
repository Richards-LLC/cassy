# Brief: `cas factory status`

| Field | Sentence |
| --- | --- |
| First two lines | Whether the session is moving — agents present, no actionable idle — and the counts that say so. |
| Scannable | Four labelled rows (Project, Queue, Tasks, Agents) in a fixed 10-cell label column; an agents ledger with the heartbeat age right-aligned. |
| Readable | Nothing; every value is a count, a name, or an age. |
| Machine output | `--json`: one object with `session`, `prompt_queue_pending`, `prompt_queue_peek`, `tasks_ready`, `tasks_in_progress`, `epics`, `agents`, `activity`. |
| Omitted | The queue peek and activity feed (in `--json`), named in the receipt. |

## Rendering decisions

- Verdicts: `✓ active`, `⚠ idle N min` (actionable-idle minutes), `⚠ no agents` with the
  remedy `cas factory spawn`.
- The old right-aligned `        Pending prompts: 0` key/value dump became left-aligned rows
  whose values start at one column.
- The agents ledger truncates only the name column, with `…`; `--full` is honoured and named.
- The `═══ Status for session ═══` banner in accent colour is gone; the verdict carries the
  session name.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-factory-status · 12 runs · 53 fail · 0 warn` — 52 contrast, 1 unicode-without-fallback.

After: `terminal-qa: PASS cas-factory-status · 12 runs · 0 fail · 0 warn · 0 allowed · docs/design/cli/captures/after/cas-factory-status/report.md`

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict line carries state, session, and counts |
| Fit | 5 | rows for the scan, ledger for the agents, flags in the receipt |
| Craft | 4 | `0 other` is a weak word for idle-or-stale agents |
| Theme safety | 5 | one coloured cell; four palettes pass |
| Machine contract | 5 | `--json` unchanged |
