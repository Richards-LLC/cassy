### Fixed — Factory

- **Answered blockers stop waking the supervisor** — Direct supervisor replies and task decision notes now retire the current worker blocker episode while its status catches up. A new blocker question gets its own deadline, and answers for another task leave the original alert armed.
