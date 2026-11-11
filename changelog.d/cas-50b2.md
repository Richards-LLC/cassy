### Added — Delivery identity

- **Every delivery records its cycle** — Was: an ordinary park left no
  per-delivery record, so each stage re-derived identity from task fields and
  live Git. Now: every anchor write records the task's delivery cycle and
  appends later tips to it; request-changes and reopen start the next cycle,
  and a completion receipt from a later cycle gets its own id.
