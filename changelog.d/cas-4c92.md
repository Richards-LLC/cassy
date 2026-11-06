### Fixed — Factory

- **Restore refused local landings** — A merge rejected by its pre-push landing check restores the exact local target from before the attempt, preserving earlier unpublished work.
- **Keep queued assignments idle** — A supervisor's audited stale-worker assignment no longer treats queued Open tasks as active work; InProgress and Blocked tasks still hold the worker.
