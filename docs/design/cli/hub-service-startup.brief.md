# Hub service publication failure

| Field | Contract |
| --- | --- |
| First two lines | Status identifies the exited hub and offers its restart command. |
| Scannable | The Tailscale Serve row says FAIL when the service cannot publish. |
| Readable | An indented, wrapped cause names the sanitized Tailscale failure; one restart command follows. |
| Machine output | `hub status --json` retains its single document, including `record.transport_warning` and `tailscale_serve.message`; service errors go to stderr. |
| Omitted | Raw Tailscale stderr remains private; the existing error classifier supplies safe causes. Retry timing stays in the service log. |

Both launchd and systemd retry a service start that exhausts the bounded
publication attempts. The failed process record keeps the cause observable
between restarts. Successful startup clears it. On Linux the service's stderr
reaches the journal; startup and exit diagnostics also remain in hub.log.

Pending and failed starts retain the last known Serve target when a legacy
runtime record is the only ownership evidence. A successful publication replaces
it; the pending record never advertises a ready public endpoint. An unrelated
mapping still fails the exact ownership comparison and remains untouched.
