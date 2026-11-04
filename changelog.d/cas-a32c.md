### Fixed — Verification

- **Keep verdicts when a sibling lands on the epic** — Was: a merged
  delivery's verification bound the supervisor checkout's HEAD as delivered
  work, so the first unrelated merge into the epic refused the finished
  verdict as "delivered commit(s) no longer reachable". Now: only the
  delivery is bound; unrelated merges keep the verdict, and a later change to
  a delivered file still asks for a fresh review.
