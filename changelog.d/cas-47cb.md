### Fixed — Factory delivery

- **Watchdog sender and completion handling** — Delivery retries name the queued sender. A worker-check completion that has reached its worker no longer triggers another watchdog wake. Closes #1782.
