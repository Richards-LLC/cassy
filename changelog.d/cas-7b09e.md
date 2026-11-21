### Fixed — Factory

- **Cancelled work releases the next task** — Starting assigned work and repairing worker queues reconcile stale leases for cancelled or closed tasks. Unfinished work still enforces one task at a time, so recovery never asks a worker to block an already terminal task.
