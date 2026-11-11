### Added — Delivery identity

- **Every delivery records its cycle** — Was: an ordinary park left no
  per-delivery record, so each stage re-derived identity from task fields and
  live Git. Now: every anchor write, including a task created or updated by
  sync, records the task's delivery cycle and appends later tips to it, and
  open deliveries parked earlier are backfilled. Request-changes and reopen
  start the next cycle, machines that sync the task number its cycles the same
  way, and a completion receipt from a later cycle gets its own id.
