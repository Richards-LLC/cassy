### Fixed — Factory

- **Repair spawn class selector** — Supervisors can pass `class_audit` with a repair worker's `task_id` and `spawn_brief`. The effective selector is validated and saved before the worker is queued, so its park proof covers the recorded failure class.
