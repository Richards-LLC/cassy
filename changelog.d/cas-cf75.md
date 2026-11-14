### Fixed — Search

- **Search during task refresh** — Conceptual searches read the committed index when another writer is refreshing it. Task reconciliation resumes when the writer is available, without removing locks or retrying the query.
