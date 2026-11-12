### Fixed — Task delivery

- **Published start bases and successor deliveries** — Worker starts use the origin target when a local epic has unpublished commits. A cancelled task's recorded unmerged commits can be delivered unchanged by its explicitly named successor, while unrelated work keeps its ownership checks.
