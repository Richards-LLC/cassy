### Fixed — Cloud queue

- **migrate-to-team stops re-enqueueing pull echoes** — Was: task and dependency rows already equal to the team's row were "enqueued" and nothing changed. Now: they are reported as already on the team and point at `cas cloud queue --purge-pull-echoes --dry-run`. (GH #1776)
