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
- **Journey runs, pre-push rows and foreground checks hold the checkout too** — Was: only a backgrounded (`&`) capped check recorded a run, so a start could
  still switch the checkout under `scripts/journey-eval.sh` (the GH #2286
  incident), the worker pre-push fast rows, or a capped check started in the
  foreground or by a harness background call. Now: each records a run bound
  to its own process for its whole life and ends it on every exit, including
  failures, HUP, INT and TERM; a killed run stops holding the checkout when its
  process dies. These runs send no completion notice, since the caller is
  already watching them. Outside a factory worker the scripts are unchanged.
