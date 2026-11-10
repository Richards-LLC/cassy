### Fixed — Factory

- **Automatic worker context respawn** — Workers can request a fresh context with a typed respawn message. Cassy queues their existing recycle path, keeps their recipe and task bindings, links the saved handoff in the resume brief, and leaves a quiet supervisor receipt. A persistent ten-minute limit prevents repeated recycles for the same worker.
