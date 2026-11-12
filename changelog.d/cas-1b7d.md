### Fixed — QA

- **Release receipts no longer mistake a supervisor's handoff for its own work** — Was:
  if a delivery's handoff list named the supervisor's shared checkout
  (`factory/<supervisor>`), recording a release review receipt was refused
  as self-review, even when the supervisor never committed to that delivery.
  Now: that handoff counts only when the branch carries the delivery's
  commits. Per-task handoffs, leases, completion receipts and commit authors
  still block a reviewer who implemented the delivery.
