# Exemplar: status screen

A factory session status (`cas factory status`). The reader's task is a two-second scan: is the
session moving, and if not, what do I do.

## Brief

| Field | Sentence |
| --- | --- |
| First two lines | Whether the session is moving (agents alive, no actionable idle) and the one number that says so. |
| Scannable | Four labelled rows with a fixed 10-cell label column; an agents ledger whose last column is right-aligned age. |
| Readable | Nothing — every value is a count, a name, or an age. |
| Machine output | `--json`: one object with `session`, `prompt_queue_pending`, `tasks_ready`, `tasks_in_progress`, `epics`, `agents`, `activity`. |
| Omitted | The queue peek and the activity feed; both are in `--json` and the receipt says so. |

## Render at 80 columns

```
✓ active · cas-src-lively-panther-31 · 2 agents · 98 ready · 11 in progress
────────────────────────────────────────────────────────────────────────────────
Project   ~/work/cas-src
Queue     0 pending prompts
Tasks     98 ready · 11 in progress · 4 epics open
Agents    2 active · 0 other · 0 actionable-idle min

agent              status  task      last seen
lively-panther-31  active  -            12s ago
golden-koala-58    active  cas-4df0      3s ago

--json for the queue peek and activity · --full for untruncated values
```

## Annotations

1. **Verdict** — `✓` is the only coloured cell on the screen; `active` is bold in the default
   foreground. On a light terminal the mark is `#1e964b` (3.8:1 on white, above the 3:1 mark floor and
   inside the band the TUI's chip contrast guard allows), on a dark one `#50c878` (8:1).
   The alternative verdicts are `⚠ idle 14 min` and `⚠ no agents`, each with a remedy line.
2. **Rows** — labels are bold words in a fixed column, values start at cell 11. Compare the
   old right-aligned `        Pending prompts: 0`, which makes the eye hunt for the colon.
3. **Ledger** — the `last seen` column is right-aligned so a `2h ago` outlier sits visibly apart
   from `12s ago`; the agent name column is the only one that truncates, with `…`, and the
   receipt names `--full`.
4. **Receipt** — muted, last, and the only place that mentions flags.
5. **Plain mode** — piped or `NO_COLOR`, the same text prints with `[OK]` for `✓`, `-` for `·`,
   `->` for `→`, and no SGR at all.

## Score

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict line carries state, session, counts; the remedy is line eight when present |
| Fit | 5 | four rows for a scan, one ledger for the agents, flags in the receipt only |
| Craft | 4 | one seam: `0 other` needs a better word than "other" for idle-or-stale |
| Theme safety | 5 | one coloured cell; gate passes four palettes |
| Machine contract | 5 | `--json` is one document with the same counts |
