### Fixed — Factory recovery

- **Separate branch push distance from global publication** — Retained-worktree reports now show commits ahead of the worker branch's own remote separately from commits on no remote. Missing tracking refs and failed probes are explicit; work published on the train stays visible when the worker's own remote still lags. Repositories without remotes label their local-target distance separately.
