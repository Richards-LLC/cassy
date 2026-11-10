### Fixed — Factory

- **Build overflow asks reach the peer machine** — When local build capacity is full, the staffing ask sent to another machine is now a short brief (task id, title, target, supervisor brief, and clipped description and acceptance criteria) instead of the whole task, so tasks with long notes are no longer refused by Cassy Cloud. A peer message longer than Cloud's 64,000-character limit is now refused before it is queued.
