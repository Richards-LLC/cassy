# Recover a dead worker's committed branch

| Field | Contract |
| --- | --- |
| First two lines | Confirm the task's committed work was published under the authenticated successor's authority. |
| Scannable | Actor, original source, successor branch, full commit and origin each occupy one labelled row. |
| Readable | A refusal names the failed identity, liveness, source or remote proof and the evidence needed to retry. |
| Machine output | Global `--json` returns one receipt object: status, task_id, actor, source_branch, branch, head and remote; no banner or colour. |
| Omitted | Original worker files and environment are absent from the new checkout; original ref and full recovery audit stay in the task's notes, while `--json` retains unabridged receipt fields. |
