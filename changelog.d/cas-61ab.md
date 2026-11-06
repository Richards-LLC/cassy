### Fixed — Factory

- **Exited processes no longer stall review cleanup** — Garbage collection recognizes failed process reads after the PID disappears, including Linux ESRCH errors. Live processes and unreadable process inventories still preserve review output.
