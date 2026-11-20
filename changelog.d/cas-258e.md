### Fixed — Cloud sync

- **Team sync stops re-sending the same tasks every cycle** — Was: since
  v4.33.0, a project that had moved to team sync re-sent every task still
  queued from its personal days on each sync cycle, and each pull saved
  another full copy of the task to the conflict log, which grew one project
  database to 22 GB. Now: the first confirmed team write clears the queued
  copy, and a pull that changes nothing but the revision number records no
  new conflict.
