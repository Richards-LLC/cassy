# Peer mailbox status

| Field | Contract |
| --- | --- |
| First two lines | The mailbox says `receiving` or `not receiving` with the current cause; an unavailable runtime states its automatic retry cadence. |
| Scannable | A short mailbox footer appears in peers and full worker status; summary worker status reads the same local health without a cloud lookup. |
| Readable | The named cause explains missing login, absent factory session, failed claim/ack, or stale daemon reporting. Login is repaired with `cas login`. |
| Machine output | These MCP tools return text content. `message_status id=peer-outbox:<id>` reports pending, retrying, sent with its cloud id, or abandoned with the last cause. It does not assert recipient delivery from a local send receipt. |
| Omitted | Tokens and message bodies never enter the health file. Cloud delivery is read separately with `message_status id=<cloud-id>`. |

The factory always starts the mailbox duty. It reloads project login and identity
on each poll. Successful polls recur every 15 seconds; unavailable polls back off
5, 10, 20, then 30 seconds. A health receipt older than 90 seconds is stale.
Supervisors without a factory session cannot receive peer messages and are refused
at send time.

Every outbound logical send has a durable local row. Its cloud key includes the
sender, recipient and row identity, so an automatic retry cannot duplicate a send
whose cloud receipt was lost. Two deliberate sends of identical text get different
rows. Transient failures return an error with the local retry handle; missing login
and permanent HTTP refusals do not promise retries. Automatic retries stop after
15 minutes, allowing an in-flight attempt to settle before abandoning its row.
A terminal sender notice survives a changed supervisor session and is deduplicated
across daemon restarts.

## Critique

Fixture integration checks cover late login through the actual daemon polling
duty, both worker status paths, all four outbox states through MCP, concurrent
retry consumers, TTL notices, and production claim/ack response shapes.
The supervisor and independent QA own the real binary against production demo
and terminal captures after this branch is integrated; these are not yet claimed.
