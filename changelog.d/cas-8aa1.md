### Fixed — Factory

- **Idle workers with queued work can take new tasks** — Assigning a task to a clean, idle worker no longer fails with "N commits behind main, ask the worker to rebase" just because another task is queued for that worker but not yet started. `task start` cuts each task's branch from its own target. A worker that is mid-task or has uncommitted changes is still refused.
