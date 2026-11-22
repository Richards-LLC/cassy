## Workflows

### Revalidating an already-landed task

When a supervisor reopens a task whose delivery already landed, the next cycle may only confirm that landed work on the current tree. That cycle can close with `no_delivery=true`. The prior cycle's retained commits are exempt only when every one of them is on the integration target. The new cycle must add no commit that names the task after its recorded start. Anything else keeps the ordinary close contract. Verification still applies. While the close waits for its verdict, the worker can start their next queued task, and Cassy resumes the close on the verdict.
