### Fixed — Factory

- **Dependency waits wake only on completion** — A blocking dependency recorded during a worker hold now suppresses repeated blocker-deadline wakes. Once all blockers close or cancel, the supervisor receives one wake naming the resolved dependencies, even after a linked answer; the explicit hold remains until cleared.
