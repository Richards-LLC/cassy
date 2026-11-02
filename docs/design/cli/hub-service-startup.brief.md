# Hub service transport states

| Field | Contract |
| --- | --- |
| First two lines | Status identifies a healthy loopback fallback or recovered publication. |
| Scannable | The Tailscale Serve row warns for local fallback and reports the recovered route. |
| Readable | An indented, wrapped cause names the sanitized Tailscale failure; automatic retry follows. |
| Machine output | `hub status --json` retains its single document, including `record.transport_warning` and `tailscale_serve.message`; service errors go to stderr. |
| Omitted | Raw Tailscale stderr remains private; the existing error classifier supplies safe causes. Retry timing stays in the service log. |

Auto services serve loopback immediately for every publication failure, including
missing CLI, sign-in, permissions, disabled Serve and foreign routes. Publication
runs in the background, retrying after 1 second and doubling up to 30 seconds.
Recovery updates the live proxy, runtime record, advertised transport and allowed
origin without restarting the hub or requiring an operator action. Each failure
keeps its cause in status and hub.log. Required mode still refuses a failed start.
Stop cancels and joins an active CLI probe before removing Cassy's owned route;
a delayed probe cannot publish after shutdown.

Pending and failed starts retain the last known Serve target when a legacy
runtime record is the only ownership evidence. A successful publication replaces
it; the pending record never advertises a ready public endpoint. An unrelated
mapping still fails the exact ownership comparison and remains untouched.
