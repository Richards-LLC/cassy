## Core Concepts

- **Local-only recovery inventory** — Without a Git remote, retained-work inventory compares commits with the task's local target, falling back to the configured base or primary checkout branch. It still shows dirty or ahead worker checkouts. Push instructions use a configured remote and are omitted for local-only repositories; inspection remains read-only.
