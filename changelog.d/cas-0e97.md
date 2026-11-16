### Fixed — Lane CI

- **Lane CI attributes failures on a branch's first push** — Before: with no
  push base and no recorded start base, "impact own tests" ran
  `git worktree add` with an empty ref, failed with `invalid reference` and
  marked every own failure unresolved. Now: attribution uses the target
  merge-base the impact plan already resolved. If no base resolves at all, own
  failures stay red with the reason, and git is never called.
