# Operator reply held during Commander reconnect

Design: DESIGN.md (Cassy voice, inherited Commander roles); plain MCP tool text.

| Field | Contract |
| --- | --- |
| First two lines | Say the Commander turn is held for delivery and saved in the operator thread; delivery resumes on reattach without another supervisor send. |
| Scannable | Keep the existing ID, answered notification, device and status rows so the held answer has the same durable identity as an immediately queued answer. |
| Readable | One short sentence explains the held state; it does not promise that the operator has received or read the answer. |
| Machine output | MCP CallToolResult remains successful text content, with the existing notification ID and routing fields; no new JSON schema or banner. |
| Omitted | The sampled client count, watchdog timings and outbox implementation stay out of the receipt; they do not change whether the turn is recorded. |

Fresh daemon status reporting zero local Commander clients and no enrolled cloud feed changes only the receipt to held. Stale, unavailable and connected status keep the existing queued wording. Valid active-session turns always enter the existing durable operator thread. Authority, active-session and reply-reference refusals remain in force.
