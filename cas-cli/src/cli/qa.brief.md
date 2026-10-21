# Journey registry check output

| Field | Contract |
| --- | --- |
| First two lines | State PASS or FAIL and the problem/debt counts, then identify the registry file. |
| Scannable | Print one ASCII row per problem or debt item, with the journey/group and stable problem code. |
| Readable | Each row explains the missing field, reference or tracked file; the final retry command is `cas qa journeys --check`. |
| Machine output | Global `--json` emits one object with `problems` and `debt` arrays of `{code, subject, message}`, without banners or colour. |
| Omitted | Successful journeys and traffic snapshots stay in YAML; checking does not execute specs. |

Rows retain complete paths and identifiers for copying; JSON is the escape for long rows.
Malformed YAML, missing registry and Git errors fail with the underlying cause.

## Critique

Real candidate binary and terminal captures are owned by epic assembly; worker
proof exercises the shared check function against real Git fixtures. Terminal
QA is NOT VERIFIED until the candidate is built.
