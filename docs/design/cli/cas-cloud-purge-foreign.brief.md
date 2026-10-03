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

Pending compiled regression output. The added line uses default foreground,
ASCII and a bounded integer count, and its longest integer form fits 80 columns.
Full-command real-build terminal QA belongs to supervisor assembly: factory
workers may compile and execute capped named tests but may not build the CLI.
