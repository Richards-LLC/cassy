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

Real candidate binary and terminal captures are owned by epic assembly; worker
proof exercises the shared check function against real Git fixtures. Terminal
QA is NOT VERIFIED until the candidate is built.
