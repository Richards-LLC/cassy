### Fixed — Peer-machine review binding

- **A peer worker's review binds its delivery** — Was: when the worker's
  checkout was on another machine and its lane was unpushed, unfetched or
  cached at an older commit, a supervisor's close bound the review to the
  supervisor's own checkout HEAD (a stale `main` or an unrelated branch) or
  to that older commit. Now: close refreshes the lane and the target from
  origin and binds the lane's pushed tip; without a verified pushed tip it
  binds the target branch tip. A lane known only as `origin/<lane>` is
  fetched as `<lane>`. A `local_merge` lane keeps its local branch.
- **A review of an unrelated head is retired, not merged** — Was: an
  approved review bound to a commit with none of the task's work made close
  answer MERGE REQUIRED for that commit, even after the real delivery had
  merged. Now: close retires that review, records why in the task notes and
  handles the task as a delivery with no reusable review. A supervisor's
  recovery close with the merged commit receipt then closes it.
