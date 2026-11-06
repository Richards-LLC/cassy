### Added — Factory

- **Ordered train merges** — Supervisors can queue a train's parked deliveries once with `factory action=train_merge id=<epic>` and an optional ordered `task_ids` list. The queue retains its position through candidate review and CI, reuses unchanged patch approvals, returns conflicting files to the affected worker, and holds single merges into that train until it finishes.
