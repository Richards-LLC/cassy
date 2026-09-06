# Brief: `cas doctor`

| Field | Sentence |
| --- | --- |
| First two lines | Whether the local store is healthy, how many findings need a hand, and which project and version this is about. |
| Scannable | One line per healthy group with its marks; one row per finding under its group, with the mark, the name, and a repeat count when the same check fired per instance. |
| Readable | Each finding's cause (at most three lines) and its remedy under `→`; everything whole under `--verbose`. |
| Machine output | `--json`: one array of `{name, status, group, message, remediation, duration_ms, phase}`; nothing else on stdout. |
| Omitted | Per-check timings, the slow-phase table, and instances beyond the first of a repeated finding — all under `--verbose`. |

## Rendering decisions

- The verdict word is the count of findings (`2 warnings`, `1 error`, `healthy`); the healthy
  count, project, and version are the detail.
- Colour touches marks only. The old renderer painted every line in its status colour, which
  on a light terminal put `#e4e5eb` text on white (1.2:1) and amber paragraphs at 2.8:1.
- A token wider than the column (a cloud scope id) is cut with `…` instead of being split
  across lines.
- Consecutive findings with the same name and status fold into one row with `×N`.
- Cause and remedy each stop at three lines with `…`; the receipt names `--verbose`.
- The receipt keeps the exact `N ok · N warnings · N errors · time` grammar scripts grep for.

Message text itself (the cause and remedy sentences) belongs to the doctor self-heal work;
this brief covers the render only.

## Critique

Before (build `eda3dfd1`): `terminal-qa: FAIL cas-doctor · 12 runs · 841 fail · 24 warn` — 788 contrast, 28 word-split, 24 overflow, 1 unicode-without-fallback.

After: `terminal-qa: PASS cas-doctor · 12 runs · 0 fail · 0 warn · 0 allowed · docs/design/cli/captures/after/cas-doctor/report.json`

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 5 | verdict is line one; each finding's remedy is its own `→` line |
| Fit | 4 | healthy groups and repeats collapse; the remaining causes are still sentences authored as paragraphs |
| Craft | 4 | no split tokens, no overflow at 80 or 120; the `×N` widens the name column for the whole group |
| Theme safety | 5 | marks only; four palettes pass |
| Machine contract | 5 | `--json` unchanged, one array |
