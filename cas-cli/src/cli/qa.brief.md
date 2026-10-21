# Journey registry check output

| Field | Contract |
| --- | --- |
| First two lines | State FAIL, DEBT or PASS with problem/debt counts, then list debt by kind when present. Debt keeps exit zero when there are no problems. |
| Scannable | Group debt by stable problem code; show growth against the previous recorded check before registry details. |
| Readable | Print one `cas qa journeys --check --verbose` command for individual debt rows; errors still name the subject and remedy. |
| Machine output | Global `--json` preserves `problems` and `debt` arrays and adds `debt_summary` with total, by_kind, previous_total, delta, grew and history_status; absent comparisons are null. |
| Omitted | Individual debt rows need `--verbose`; successful journeys and traffic snapshots stay in YAML. Checks do not execute specs. |

Verbose rows retain complete paths and identifiers; JSON is the escape for long rows.
The existing cas.db metadata table stores one check receipt per checkout and registry.
No database is created by checking. Unavailable history is explicit and never means zero.
Malformed YAML, missing registry and Git errors fail with the underlying cause.

## Critique

terminal-qa: PASS qa-journeys-check · 12 runs · 0 fail · 0 warn · 0 allowed

Receipt: task `cas-cb2b`, `terminal-qa/qa-journeys-check/report.json`.
The candidate comes from the capped CLI integration test, as authorized by the
supervisor. Final delivery evidence binds the candidate version and receipt to
the delivered commit.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | DEBT and counts lead; line two introduces the kind breakdown, then growth and one details command follow. |
| Fit | 5 | Repeated instances collapse into kind counts; individual rows remain available with --verbose. |
| Craft | 4 | Seven short lines, at most 43 cells, fit both widths without truncation. |
| Theme safety | 5 | Four palettes pass; the 80-column dark/light HTML and plain captures are readable. |
| Machine contract | 5 | One JSON document preserves both arrays and supplies typed counts and nullable comparisons. |

Scored by true-otter-3 on 2026-10-21; all floors hold.
