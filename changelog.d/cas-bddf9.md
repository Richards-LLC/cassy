### Added — Factory

- **An unanswered peer question holds the merge it is about** — When a supervisor asks a peer supervisor a question (`kind=ask`, or a message sent with `blocker=true`) that names a task or PR, Cassy now refuses that supervisor's `worktree_merge` of the task, or its `gh pr merge` of the PR, until the peer replies. To merge anyway, the supervisor adds a task note `PEER ASK OVERRIDE <ask id>: <reason>`. Cassy then lets the merge through, counts it in the friction ledger and sends the operator a one-line notice.
