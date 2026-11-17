### Fixed — CI

- **A lost lane impact run fails with its plan** — Was: when the hosted runner for "Scoped Validation — impact own tests" stopped responding, the job ran 55 minutes, uploaded no log and ended "cancelled", so every such lane needed a manual rerun and read as unknown. Now: a dependent "Scoped Validation — impact overrun" job fails and names the plan and the elapsed time against the 45-minute budget; a frozen-train repair treats it like the other impact jobs.
- **Whole-workspace plans leave the full suite to the train** — Was: a lane whose impact plan widened to the whole workspace reran the full suite as up to three ring shards. Now: the lane runs that plan's own tests only, and the train's full set runs the rest.
