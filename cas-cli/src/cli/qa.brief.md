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

## Integration upkeep status and enforcement

| Field | Contract |
| --- | --- |
| First two lines | Registry coverage verdict, then the measured integration ref as origin/target@sha. |
| Scannable | Three artifact rows show CODEMAP structural changes, overview relevant changes, and registry validation/ownership issues; curation debt stays separate. |
| Readable | Config saves warn only when a gate changes into enforce with positive measured drift; they name the artifact, count and status command. |
| Machine output | --status preserves coverage fields and adds upkeep; config set preserves key/old_value/new_value and adds warnings in the same JSON document. Unknown drift is null, never zero. |
| Omitted | No new qa status command, remote fetch, or persistent drift counter; individual freshness commands remain available. |

Journey checks fail on malformed YAML, missing registries and Git errors,
naming the underlying cause.

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

## Measured ref follow-ups (cas-1db78)

| Field | Contract |
| --- | --- |
| First two lines | Upkeep status still leads with its verdict, then identifies the measured integration ref and commit. |
| Scannable | An over-wide branch keeps both branch ends and its measured `@sha7`, with the middle elided within 80 columns. |
| Readable | An enforce advisory sends the operator to plain `cas qa journeys --status`; the full-ref escape remains `--status --json`. |
| Machine output | Status and config-save JSON preserve the complete measured ref and existing typed drift fields in one document. |
| Omitted | The unpinned live-store config fallback is documented in source; no new status fields or config behavior are added. |

## Upkeep critique (cas-a77e)

terminal-qa: PASS qa-upkeep-status · 12 runs · 0 fail · 0 warn · 0 allowed

terminal-qa: PASS qa-enforce-warning · 12 runs · 0 fail · 0 warn · 0 allowed

Receipts: task `cas-a77e`, `terminal-qa/status/report.json` and
`terminal-qa/enforce/report.json`. The actual CLI inherits each test's terminal;
Git fixture setup is captured separately. Long integration refs exercise the
80-column truncation and full-ref JSON escape. Both 80-column four-palette
renders were inspected. Final delivery evidence records the candidate version,
mtime and commit; the candidate comes from the capped CLI integration test.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | DRIFT leads status, followed by the measured ref and three artifact counts. A saved config leads its advisory warning. |
| Fit | 5 | Three aggregate rows expose drift while curation debt stays separate; JSON carries full refs and diagnostic detail. |
| Craft | 4 | Long refs truncate visibly at 80 columns and name the JSON escape; both command surfaces remain readable. |
| Theme safety | 5 | Both surfaces pass all four palettes and plain-output checks; inspected renders retain clear labels and counts. |
| Machine contract | 5 | One JSON document preserves existing fields; unknown counts are null and config warnings are an explicit array. |

Scored by true-otter-3 on 2026-10-21; all floors hold.
