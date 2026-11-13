### Fixed — Factory

- **Publish epic lanes on first child assignment** — Creating or assigning a child to a worker, or starting it, now checks and publishes its local epic branch even when the worker was spawned without a task. Existing remote branches, recorded ownership and explicit child targets remain protected.
