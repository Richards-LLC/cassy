### Fixed — Task start

- **A reassigned task rebinds its source branch when the new worker starts it** — Was: after a task was transferred or reassigned, `task start` kept the previous worker's recorded source branch and warned, so the new worker had to ask the supervisor to run `rebind_branch`. Now: the new worker's first start records their own task branch and base as the source; repeated starts and same-worker retargets are unchanged.
