### Fixed — Factory workers

- **Temporary worker branches keep their registered owner** — Was: a worker
  on `factory/<worker>-01ea` or a review suffix could be rejected as a different
  worker, while an overlapping worker's task branch could be mistaken for its
  own. Now: ownership resolves against registered worker namespaces, preferring
  the most specific registered name. Git guards, binding checks and task
  start/adoption/close use that ownership while preserving checkout containment
  and recovery to the worker's own branch.

- **Git hook ownership matches the factory guard** — A
  factory/<worker>-<suffix> branch is the worker's own unless a registered
  worker of that full name exists. Installed pre-commit and pre-push hooks now
  consult the same registered ownership as H-22, including longer sibling
  namespaces. Preservation refs still require an explicit matching source and
  destination; a sibling's task ref remains protected. The cas-b153 fixture now
  registers its overlapping sibling to preserve that intended rejection.
