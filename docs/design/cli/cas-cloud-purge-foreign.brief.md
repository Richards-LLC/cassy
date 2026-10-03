# Purge foreign rows: queued changes

| Field | Contract |
| --- | --- |
| First two lines | The existing purge preview identifies the operation and project; this scoped change reports preserved queue rows alongside the existing before-counts, so an unrelated queue is never presented as a refusal. |
| Scannable | One plain ASCII line, `Info: N unrelated queued change(s) preserved`, gives the count; classifier and sync freshness refusals retain their existing group. |
| Readable | The queue exemption is explained in the command documentation: classified replicas are deliberate cleanup, and unrelated queue rows survive deletion. |
| Machine output | Dry-run and successful apply JSON add the numeric `non_overlapping_queued_changes` field; stdout remains one document with no new banner or color. |
| Omitted | Queue payloads, credentials and per-row IDs are excluded from this informational count; `cas cloud queue` remains the inspection command. |

## Scope and verification

The count covers every queued entity kind after excluding typed delete-set IDs
and dependency edges touching deleted tasks. Queue read/schema/decode failures
still abort the preview. Freshness, majority and proven-rule guards retain their
force rules. Fixtures run under temporary HOME and store paths; no command may
purge the operator store.

## Critique

The scoped compiled regressions pass: 51 purge-safety and doctor queue tests.
The incident fixture exercises the real inspection and deletion paths, reports
364 preserved rows and keeps the queued work. The snapshot asserts the exact
ASCII info line and checks zero, 364 and the largest integer at 80 columns.
The human caller writes that helper in the default foreground; dry-run and apply
JSON both serialize the same inspection count as a number.

Full-command terminal QA is **not verified** here. It belongs to supervisor
assembly against the freshly built CLI and a disposable store: factory workers
may execute capped named tests but may not build the CLI. The existing command
header and unmodified report layout have not been scored or redesigned as part
of this queue-safety fix.
