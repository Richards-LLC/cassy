### Fixed — Factory

- **Worker work remains visible after supervisor exit** — The terminal and session-end receipt now retain each worker's tasks, dirty and unpushed work and stop outcome. Confirmed stopped workers release unfinished task bindings for recovery instead of leaving them assigned to an unregistered worker.
