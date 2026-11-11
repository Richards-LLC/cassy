### Fixed — Factory

- **Resume after worker recycle** — Recycling releases the old session's task leases after stopping its process, so a task start finishing during shutdown cannot block the replacement worker.
