### Fixed — Factory workers

- **Temporary worker branches keep their registered owner** — Was: a worker
  on `factory/<worker>-01ea` or a review suffix could be rejected as a different
  worker, while an overlapping worker's task branch could be mistaken for its
  own. Now: ownership resolves against registered worker namespaces, preferring
  the most specific registered name. Git guards, binding checks and task
  start/adoption/close use that ownership while preserving checkout containment
  and recovery to the worker's own branch.
