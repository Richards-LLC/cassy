# cas qa fixtures --sweep

| Field | Contract |
| --- | --- |
| First two lines | The first line gives the cleanup verdict and counts; retained debt adds one retry command on line two. |
| Scannable | One compact summary shows cleaned handles and orphan debt, without one row per successful teardown. |
| Readable | The retry command is `cas qa fixtures --sweep`; `--json` provides each retained cleanup ID and cause for investigation. |
| Machine output | `--json` emits one object with numeric `cleaned` and `orphan_debt: [{cleanup_id, error}]`, with no human banner. |
| Omitted | Handles, recipe outputs, adapter stdout and per-call transcripts stay off the human screen; JSON includes only cleanup IDs and sanitized errors. |

The output uses ASCII marks and no colors or cursor controls, so pipes, narrow
terminals, light/dark themes and C locales carry the same meaning. Teardown
debt is reported without modifying any suite verdict. A sweep with retained
debt exits unsuccessfully after printing its report.

## Critique

terminal-qa: PASS cas-qa-fixtures · 12 runs · 0 fail · 0 warn · 0 allowed

terminal-qa: PASS cas-qa-fixtures-orphan · 12 runs · 0 fail · 0 warn · 0 allowed

Captures and the full receipt are in the cas-85a6 task artifacts under
`terminal-qa/report.json` and `terminal-qa-orphan/report.json`;
`cli-sweep-proof.log` records failed teardown (exit
1 with retained debt), JSON failure, then successful retry (exit 0, one cleaned
handle, no remaining row). These captures use the binary produced by the
passing named `doctor_snapshot` integration run at `ac83dc23f`.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first line gives cleaned/debt counts; the warning's second line names the retry command, followed by the CLI's standard error diagnostic. |
| Fit | 5 | All 80/120-column captures fit; the healthy line uses 39 cells. |
| Craft | 4 | One summary replaces per-handle noise; the warning offers a JSON detail command. |
| Theme safety | 5 | Four palettes, piped, NO_COLOR and C-locale runs pass without color dependence. |
| Machine contract | 5 | One typed JSON document; incomplete cleanup exits 1 and diagnostic text stays on stderr. |

Scored by mighty-dolphin-57 on 2026-10-21. The task permits only the loader,
cleanup and migration regressions; the existing doctor snapshot is the CLI
snapshot control. Captures stay in the durable task artifact directory.
