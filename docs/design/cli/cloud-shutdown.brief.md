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

## Critique

terminal-qa: PASS cloud-shutdown-footer · 11 runs · 0 fail · 0 warn · 0 allowed

The capture is the committed footer fixture, checked byte-for-byte against the
real renderer by `cas_4ac8_shutdown_health_footer`. QA covers this footer, not
an entire `cas status` screen. Full captures and the receipt are in the task's
`terminal-qa/` artifact directory.

| Dimension | Score | Evidence |
| --- | ---: | --- |
| Hierarchy | 4 | Counts lead; automatic retry or the inspection command follows immediately. |
| Fit | 5 | Repeated failures collapse to counts; completed cleanup is omitted. |
| Craft | 4 | Existing 72-column hanging wrap; capture maximum is 51 columns. |
| Theme safety | 5 | All four palettes, 80/120 columns, pipe, C locale and NO_COLOR pass. |
| Machine contract | 4 | Existing JSON message field carries the same footer; no schema addition. |

Reviewed by calm-viper-79 on 2026-10-16. Release impact: rate-limited SessionEnd
cleanup survives process exit, and presence/mailbox health explain the wait.
