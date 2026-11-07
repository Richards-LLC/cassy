### Fixed — Factory

- **Releasing a queued task lets the queue move on** — Was: `task release` on a worker's current queue task reset it to Open, but the daemon's queue repair restarted it at once, so the next task's start refused with "has active work". Now: release drops the task from its worker's queue and starts the next pending task; the released task stays Open and unassigned.
