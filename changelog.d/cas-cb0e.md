### Fixed

- `cas cloud queue migrate-to-team` no longer enqueues personal task and dependency rows that already equal the team's current row. It reports them as already on the team and points at `cas cloud queue --purge-pull-echoes --dry-run`; the held-queue remediation text names the purge too (GH #1776).
