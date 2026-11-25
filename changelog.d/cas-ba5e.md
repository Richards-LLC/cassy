### Fixed — Factory

- **Repeated rejections require a whole-flow review** — Was: a task could cycle through narrow repairs and hold its release train. Now: after two all-time verifier rejections, Cassy requires a linked whole-flow review before another repair, assignment, or start and detaches the task unless a supervisor records why it should stay. Detachment preserves the recorded source branch, and the daemon recovers missed notices.
