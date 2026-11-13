### Fixed — Factory

- **No false worker_stalled during a long capped build** — Was: a Claude worker's foreground `worker-check` build read as silent and paged the supervisor at 10 minutes, because its child processes were never matched to the worker. Now: child CPU and output count as activity for Claude workers. (cas-8ea3)
