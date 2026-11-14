### Fixed — Factory

- **QA can be requested for a rebound re-delivery** — Was: after a
  supervisor rebound a task to a new branch such as
  `factory/<worker>-<task>-redelivery`, `verification qa_request` with that
  branch's pushed tip was refused as "not the live pushed tip", so the close
  stayed stuck. Now: the task's recorded delivery branch from the rebind is
  checked too, and the round opens at its live origin tip.
