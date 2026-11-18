### Fixed — QA

- **Batch evidence stays with the batch** — Worker parks honor resolved `qa.mode=batch` when target ancestry is unknown, instead of requesting a per-task QA bundle. Proven integrated deliveries retain their final-close evidence path; per-task evidence and final-close batch receipt requirements remain enforced.
