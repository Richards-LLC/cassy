### Fixed — QA batch receipts

- **batch_receipt accepts PR-merged tasks** — Was: a task merged through a GitHub PR had no recorded anchor and batch_receipt refused it. Now: it resolves the task's branch tip (or its squash commit) on the target, as close's commit_receipt does.
