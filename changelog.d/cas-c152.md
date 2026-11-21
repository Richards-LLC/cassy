### Fixed — Factory workers

- **A detached check run from a worker's package folder or its task scratch now wakes the worker** — Was:
  a `cas factory worker-check --detach` started anywhere other than the exact
  root of the worker's checkout, such as `hub-web` inside it or the task's own
  scratch checkout, was not tracked, so the worker was never told it finished.
  Now: runs inside the worker's checkout, and runs in a scratch checkout owned by
  the worker's task, are tracked and wake the worker once when they finish.
