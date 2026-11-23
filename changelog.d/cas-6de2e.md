### Fixed — Director and watchdog notices

- **No stall alarm while a worker waits on its own reminder** — A worker whose task is in progress and who has armed its own reminder for that task is treated as waiting, not stalled. The director no longer nudges it or tells the supervisor to recover it every ten minutes. If the worker is still quiet after the reminder fires, the usual stall nudge and escalation resume (GH #1728).
