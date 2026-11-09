### Fixed — Factory

- **Tasks assigned during worker startup stay queued** — Assigning later lane tasks before a worker registers now preserves their order and delivers the next task after the current delivery. Repeated dispatch does not duplicate work, and canceled or reassigned tasks are skipped.
