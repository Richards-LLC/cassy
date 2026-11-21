### Fixed — Peer-machine review binding

- **A peer worker's review binds its delivery** — Was: when the worker's
  checkout was on another machine and its lane was unpushed or not yet
  fetched, a supervisor's close bound the review to the supervisor's own
  checkout HEAD (a stale `main` or an unrelated branch). Now: close fetches
  the lane, binds its pushed tip when present, and otherwise binds the
  target branch tip.
- **A review of an unrelated head is retired, not merged** — Was: an
  approved review bound to a commit with none of the task's work made close answer MERGE REQUIRED for that commit, even after the real
  delivery had merged. Now: close retires that review, records why in the
  task notes and handles the task as a delivery with no reusable review. A
  supervisor's recovery close with the merged commit receipt then closes it.
