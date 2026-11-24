### Fixed — Factory delivery

- **Retargeted lanes keep their reviewed history** — Was: a task laned off a
  release train, then moved to the successor train, was refused at close
  because that train's already-reviewed commits counted as foreign work. Now:
  history from an earlier epic or release-train target counts as upstream;
  other branches stay foreign, and a deleted target is named as missing.
- **Adopted lanes own their delivery** — Was: `transfer adopt_branch` left the
  start record on the old worker's lane, so start, rebind and close measured
  the wrong branch. Now: adoption records the receiver's lane and base.
- **The no-branch close refusal gives a push the guard allows** — Was: it told
  the worker to push to a per-task ref that the push guard refuses. Now: it
  names the per-task branch locally, then pushes it under the same name.
