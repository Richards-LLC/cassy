# Cloud shutdown health footer

| Field | Contract |
| --- | --- |
| First two lines | The existing presence footer names pending and expired cloud shutdown counts, then the automatic retry or inspection action. |
| Scannable | One count row follows current-agent presence; completed intents disappear, and failed intents share one cause row. |
| Readable | The cause row names rate limiting explicitly; an expired intent directs the operator to inspect its durable record. |
| Machine output | Existing peers JSON keeps `cloud_presence` for the caller and includes this footer in `cloud_presence_message`; no new top-level fields or banners. |
| Omitted | Tokens and agent metadata never enter intents; only up to three bounded cause examples appear, with complete records under `.cas/cloud_shutdown/`. |

The footer uses existing 72-column word wrapping, plain ASCII and the terminal's
normal foreground. This changes health copy, with no new command, table,
terminal renderer, projection or hook registration.
