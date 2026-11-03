# Account mutation help

| Field | Contract |
| --- | --- |
| First two lines | Rename/remove help keeps the command's purpose and usage first. |
| Scannable | A shared macOS footer names the affected shells, selectors and the action to take. |
| Readable | Close shells that retain the account selector before changing the account; the busy guard cannot reliably inspect those shells. |
| Machine output | Help remains plain CLI help; mutation JSON receipts and exit codes are unchanged. |
| Omitted | Kernel API evidence and the stale-shell consequence belong in CONTRIBUTING, rather than command help. |

design: DESIGN.md@b43940e43

## Critique

The footer uses identical wording on both commands, explicit line breaks within
80 columns, and no new colour or terminal controls. Real-build terminal captures
and their scores are recorded in the task's evidence ledger after candidate QA.
