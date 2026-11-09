### Fixed — Factory

- **Resolve lane models for the chosen account** — Codex lanes now check the selected account before queueing and warn when choosing its supported registered fallback. Missing or stale catalogue evidence refuses the lane before creating a worker or worktree; explicit unsupported recipes still refuse. Workers with a terminal authentication or model failure are excluded from idle staffing until they recover.
