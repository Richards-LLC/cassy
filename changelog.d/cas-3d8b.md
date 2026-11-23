### Fixed — Cloud sync

- **A stale quarantine entry no longer hides a later local task from sync** — Was: a quarantine entry whose short task id was later reused by a task this project created made every pull skip that task's peer updates and deleted its queued pushes, so a peer's park stayed Open on the train owner's machine. Now: an entry older than a local task stamped with this project is ignored by pull, push and the board, while rows that existed when they were quarantined stay quarantined.
