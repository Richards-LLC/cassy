### Fixed — Factory repair briefs

- **Lane failures require a class audit** — Was: only merge-queue ejections
  recorded named failing tests for the repair gate. Now: the CI watcher reads
  the existing lane-impact attribution receipts for exact parked delivery
  heads, including train targets, and records only their `caused` test names.
  Inherited failures, stale receipts and log wording do not trigger the gate.
  A decline still requires a covering class selector and passing class proof
  before re-park. Verifier findings remain a separate follow-up.
