### Fixed — Factory

- **Reexec no longer refuses behind a pile of reconcile reads** — Was: when the
  external job lane stayed busy for minutes, the worker-task-queue reconcile read
  was resubmitted every 30 s and each copy queued as durable work (452 after
  3.8 h), so `cas factory reexec` refused and only a full restart helped
  (GH #2327). Now: a resubmit replaces the one waiting, and reexec abandons it
  like other re-runnable reads.
