### Fixed — Tasks

- **Standalone task creation avoids a nested train ID collision** — Opening a release train now finishes before the joining task receives its ID, preventing the train from taking the task's pending ID.
