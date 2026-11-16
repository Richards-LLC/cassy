### Fixed — Factory delivery

- **Recorded task commits stay the task's own** — Was: a commit the worker
  made for its task, but whose subject named another task, was refused as
  foreign at start and close. A supervisor's `merge_source` grant was ignored
  at task start and inside a `--no-ff` import, so rebind, start and close
  needed a history rewrite. Now: commits Cassy recorded in the task's delivery
  cycle, and commits a grant adopts, count as the task's work at start, rebind
  and close. Unrecorded, ungranted work is still refused.
- **Returned tasks name their recorded branch** — Was: a returned task's start
  checklist and close checks could measure an empty `factory/<worker>-<task>`
  ref instead of the recorded lane. Now: that ref is used only when it carries
  the task's delivered work.
- **Parent links carry an unchanged empty source** — Was: retargeting a task
  through a `dep_add` parent link left its start on the old target, so the
  no-delivery close was refused G-47. Now: `dep_add` rebinds the unchanged
  empty source exactly as `task update epic=` does.
