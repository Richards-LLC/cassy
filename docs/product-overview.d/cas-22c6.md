## Core Concepts

A task start, queued or direct, never moves a worker's checkout while it is busy. A start that would switch, cut or reset the checkout waits with code G-165 while the worker has a live detached run or uncommitted changes. A queued start proceeds on a later queue pass once the checkout is clear. Journey runs, worker pre-push rows and foreground capped checks register a run bound to their own process, so they hold the checkout for their whole life. A start that leaves the checkout as it is, such as resuming on the task's own branch, is not held.
