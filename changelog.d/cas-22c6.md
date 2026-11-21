### Fixed — Factory task queue

- **A task start waits instead of switching a busy checkout** — Was: when a
  worker parked or released a task, the queue started its next task at once
  and switched the worker's checkout to the new branch, even while a detached
  proof run (a journey or capped test run) was still reading the old tip, so
  that run's receipt was refused; a direct start did the same. Now: a start
  that would actually move the worker's checkout (a branch switch or cut, or
  a reset of a reused lane), queued or direct, waits while the worker has a
  live detached run or uncommitted changes, with code G-161. A start that
  leaves the checkout as it is, such as resuming a task on its own branch,
  is not held. A queued start proceeds on a later queue pass once the
  checkout is clear.
