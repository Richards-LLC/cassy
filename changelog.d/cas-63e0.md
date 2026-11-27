### Changed — Release train

- **An open or frozen release train no longer holds main** — Was: while a release train was open, `worktree_merge` refused every merge into main, and a freeze also refused new staffing, worker spawns and CI retry arming on every machine. Now: only a merge into a cut or frozen train's own branch is refused (a `release-repair` task still merges while the train is red); merges into main or any other branch, task starts, spawns and CI retries ignore the freeze.
