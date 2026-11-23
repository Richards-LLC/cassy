### Fixed — Revalidating an already-landed task

- **Revalidation closes without a new delivery** — After a supervisor reopens a task whose delivery already landed, a worker that only revalidates it can close with `no_delivery=true`. The close is still refused if any retained commit is missing from the target, or if the new cycle added commits that name the task. Verification still applies (GH #1695).
- **The worker's queue keeps moving** — While that close waits for its verdict, the worker can start their next queued task. Cassy resumes the close on the verdict without the worker.
