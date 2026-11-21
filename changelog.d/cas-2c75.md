### Fixed — Factory daemon

- **Shutdown no longer skips the final cloud sync** — Was: a shutdown during a
  background sync aborted it while it still held the store's sync lease, so the
  final sync found the lease busy and printed "Final cloud sync complete (nothing
  pending)" without syncing, and quality telemetry shared its 10 s bound. Now:
  shutdown lets the in-flight sync finish, runs the final sync, then quality
  telemetry outside that bound.
- **A reused worker lane starts from its moved epic** — Was: a lane whose only
  extra commits had already shipped on trunk resumed on its old base without the
  epic's newer commits. Now: it resets to the epic tip.
- **A merge retried after its target moved builds against the new target** — Was:
  it could join a candidate build still running for the old target tip. Now:
  builds are shared only for the same source and target commits.
- **Explicit syncs say when they were deferred** — Was: `cas cloud sync` and
  `cas ledger report` silently showed stale knowledge and quality data when
  another round held their lock. Now: they print that the round was deferred.
