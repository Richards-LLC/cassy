### Fixed — Cloud sync

- **A stale quarantine entry no longer hides a live local task from sync** — Was: a quarantine entry whose short task id matched a task this project owns made every pull skip that task's peer updates and deleted its queued pushes, so a peer's park stayed Open on the train owner's machine. Now: an entry that names a task stamped with this project is ignored by pull, push and the board, while ids with no local owner stay quarantined.
