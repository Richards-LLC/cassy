### Fixed — Cloud sync

- **Working task ownership** — A live owner's task restored to InProgress after a blocked yield stays protected from Open or reassignment snapshots even without a lease. Newer same-owner blocked and terminal updates still apply, and peers can recover tasks whose owner has exited.
