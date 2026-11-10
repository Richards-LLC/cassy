### Fixed — Violet connection recovery

- **Recover without restarting the session** — Violet reconnects on the next call after a transport close. Message posts carry a request ID; uncertain outcomes are reconciled by that ID without sending the message again.
