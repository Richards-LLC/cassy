### Fixed — Cloud sync

- **Review verdicts reach the machine that owns the task** — Was: when a
  supervisor on one machine ran `request_changes` on a task parked on another
  machine, the change was never queued for sync. The owning machine kept the
  task awaiting merge, so its worker could not restart. Now: `request_changes`,
  reopens and proof-scope corrections are queued like any other task write,
  so the owning machine sees the task reopened after its next sync.
