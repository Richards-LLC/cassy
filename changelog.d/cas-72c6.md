### Fixed — QA

- **Batch evidence stays with the batch** — Worker parks honor resolved `qa.mode=batch` when target ancestry is unknown or a delivery has already landed, instead of requesting a per-task QA bundle. Per-task evidence and final-close batch receipt requirements remain enforced.
