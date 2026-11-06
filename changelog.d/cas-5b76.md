### Fixed — Factory

- **Hand GitHub sources to a peer** — A source's supervisor can use `factory action=release_claim` with its GitHub URL before a handoff. The old heartbeat stops renewing that source, and the receiver can assign the task without an override.
