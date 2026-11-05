### Fixed — Factory

- **One idle wake per delivery state** — The director no longer re-sends the same worker-idle wake every few seconds once a worker's delivery has merged. A read that misses the worker's state no longer counts as a change, and the last wake sent is saved so a daemon restart does not send it again. A finished background job is reported once. A task that has left the merge queue is no longer described as "close rejected (MERGE REQUIRED)".
