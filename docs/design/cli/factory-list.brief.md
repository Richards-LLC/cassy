# Factory session lifecycle labels (cas-c636)

| Field | Contract |
| --- | --- |
| First two lines | The existing session heading leads into rows; an absent or replaced daemon is explicitly `dead`, rather than suggesting a deliberately stopped session. |
| Scannable | Each existing row keeps its session name, worker count and PID, with a shared `running`, `orphaned`, `starting` or `dead` label. |
| Readable | An explicitly named dead session can be removed with `cas kill NAME --force`; failed shutdown names `cas list --name NAME` and retains live sockets. |
| Machine output | `cas list --json` remains one document and adds `sessions[].status`, derived from the same lifecycle method as terminal rows; existing booleans remain. |
| Omitted | Socket ownership checks stay internal; no kernel inventory or cleanup chatter is printed in status output. |

This is a lifecycle-label correction within the existing renderer. Layout,
palette, filtering and headings retain their current contracts.

Socket reclamation on Linux requires an owned canonical socket, no live PID
claim, no kernel socket holder, and unchanged device/inode identity. A shared
transition lock serializes binding with cleanup. Missing or ambiguous receipts,
symlinks, hard links, regular files and foreign owners are preserved. The
read-only status path does not perform cleanup. Other platforms retain socket
paths unless their daemon removes its own path at shutdown; orphan cleanup
fails closed without an authoritative holder inventory.

## Proof ownership

The worker runs isolated lifecycle/JSON/kill regressions and the capped Rust
compile check. Native CLI terminal captures and the terminal-qa critique belong
to supervisor assembly, which builds the CLI. An installed older binary cannot
prove this source's renderer; no such receipt is claimed here.
