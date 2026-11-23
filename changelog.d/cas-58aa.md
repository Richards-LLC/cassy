### Fixed — Director and watchdog notices

- **Task-scoped watchdog notices retire once their task is closed** — A watchdog notice about a specific task, such as the recovery notice for a merged delivery, now carries that task in its header. The supervisor's inbox drops it as stale when the task is already closed or cancelled, instead of asking for recovery on finished work minutes later (GH #2201, GH #1832).
