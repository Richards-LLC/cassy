### Fixed — CI

- **The stale-queue watchdog leaves backlog alone** — Was: the watchdog cancelled any non-merge-queue run that had waited more than 20 minutes, so when the lane runners were all busy it cancelled most factory CI. Now: a run that waits behind a busy runner able to take its jobs stays queued. A run whose runner labels have no runner at work is still cancelled after 20 minutes, and a re-run's age still counts from its own attempt.
