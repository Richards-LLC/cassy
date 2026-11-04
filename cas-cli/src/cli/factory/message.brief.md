# Factory message destination

| Field | Contract |
| --- | --- |
| First two lines | The success line names the message ID, actual recipient and queue session; conflicting selectors fail before any write. |
| Scannable | Preserve the existing single success line so the session can be checked at a glance. |
| Readable | A destination conflict names the explicit flag, supplied path and selected session's store, followed by the correction. |
| Machine output | `--json` returns one document with `schema_version`, `session`, `target`, `enqueued`, `message_id` and optional `ack_event_id`; session and ID correspond to the queued row. |
| Omitted | Sender environment and message bodies stay out of the receipt; conflict diagnostics carry the relevant destination paths. |

design: DESIGN.md (inherits hub-web/DESIGN.md)
