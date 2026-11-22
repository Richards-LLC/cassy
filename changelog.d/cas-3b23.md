### Fixed — Factory

- **First-prompt startup failures surface** — A registered Claude worker that never takes its first prompt now fails after the five-minute startup grace and sends a task-bound recovery notice. Launch-bound transcript activity keeps a progressing worker alive; task bindings are released only after confirmed PTY teardown.
