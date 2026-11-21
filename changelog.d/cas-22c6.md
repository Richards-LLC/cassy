### Fixed — Factory task queue

- **A queued task waits instead of switching a busy checkout** — Was: when a
  worker parked or released a task, the queue started its next task at once
  and switched the worker's checkout to the new branch, even while a detached
  proof run (a journey or capped test run) was still reading the old tip, so
  that run's receipt was refused. Now: the start waits while the worker has a
  live detached run or uncommitted changes, records why with code G-161, and
  proceeds on a later queue pass once the checkout is clear.
