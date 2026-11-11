### Fixed — Commander

- **Wildcard reply receipts** — Supervisor mirrors addressed to every device now settle when a paired device saves them. Forwarding attempts and retries survive daemon restarts, and a receipt that hits SQLite contention retries off the daemon loop.
