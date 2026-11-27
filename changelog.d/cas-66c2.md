### Fixed — Factory

- **Blocker delivery warnings no longer repeat an observed event** — The watchdog skips a stale replay when the same blocker occurrence already reached the same supervisor session. Blockers that never reached the supervisor can still raise a warning.
