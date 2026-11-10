### Fixed — Factory

- **Shutting down a booting worker is not a crash** — A worker the supervisor stops before it finishes starting is recorded as stopped, with no "failed during boot" death notice or respawn advice.
