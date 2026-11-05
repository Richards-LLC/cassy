### Fixed — Factory

- **Less wasted CI from the factory** — A gabber audit of 1,000 CI runs found three Cassy-shaped sources of waste:
  - **Epic PRs open as drafts.** Opening an epic PR without `--draft` is now refused, because every merge into a ready epic PR reran its full CI (43% of gabber's CI). The release PR is the gate. GitHub still runs CI on drafts unless the project's workflow skips them.
  - **Workers run the project's local gate before pushing.** `factory.worker_local_gate` (for example `pnpm ci:local`) now runs before a worker's push leaves the machine, so lint and test failures stop there instead of costing a CI run. It runs nothing unless set.
  - **Rapid merges get a batching notice.** After 3 or more merges into one branch within 30 minutes, `worktree_merge` reports a batching notice. Each such push reruns the branch PR's CI and cancels the previous run.
