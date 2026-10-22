# Recover a dead worker's committed branch

| Field | Contract |
| --- | --- |
| First two lines | Confirm the task's committed work was published under the authenticated successor's authority. |
| Scannable | Actor, original source, successor branch, full commit and origin each occupy one labelled row. |
| Readable | A refusal names the failed identity, liveness, source or remote proof and the evidence needed to retry. |
| Machine output | Global `--json` returns one receipt object: status, task_id, actor, source_branch, branch, head and remote; no banner or colour. |
| Omitted | Original worker files and environment are absent from the new checkout; original ref and full recovery audit stay in the task's notes, while `--json` retains unabridged receipt fields. |

## Critique

terminal-qa: PASS factory-recover-branch-receipt-fixture · 11 runs · 0 fail · 0 warn · 0 allowed

The gate used the pinned receipt fixture, whose equality is asserted by the
public factory-action regression after normalizing only its commit SHA. This is
fixture evidence for terminal layout. A real CLI binary walk belongs to the
supervisor's exact-tip candidate or assembly; no installed-binary claim is made.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Hierarchy | 4 | The first two lines state recovered task and publishing authority. |
| Fit | 4 | Source and successor refs remain explicit; the same action records the audit. |
| Craft | 4 | Labelled ASCII rows retain the full SHA and name the JSON escape. |
| Theme safety | 5 | Eleven palette/width/pipe/locale captures have no failures or warnings. |
| Machine contract | 4 | A serialized typed receipt backs global `--json`; CLI walk is deferred. |

Worker self-critique, 2026-10-22. Runtime matrix: registered role, live owner,
retired-but-live process, moved source, unrelated source, remote expectation,
successful bare-remote publication and an idempotent replay.
