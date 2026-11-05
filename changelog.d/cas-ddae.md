### Added — Factory

- **Progress watchdog** — A factory session with open work and no task, lane or message change for 45 minutes now wakes its supervisor once with what is waiting on whom. If nothing moves in another 45 minutes, the operator is paged once in Commander. Heartbeats and liveness never count as progress, so a live but silent peer no longer stalls a factory overnight unnoticed.

### Changed — Factory

- **Plain time reminders** — `coordination action=remind remind_delay_secs=<n>` is no longer refused when its message mentions CI, a task, a merge or the inbox. A time reminder is a valid backstop; event reminders remain the better wait.
