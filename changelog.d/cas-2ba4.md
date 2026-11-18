### Fixed — Factory supervision

- **Wake supervisors for ready trains and stalled work** — Owned release trains stay actionable after their last child merges, including older trains with an explicit owner and release-train branch. The watchdog repeats its wake on the stall cadence and records the promised operator page while preserving freezes and ownership boundaries.
- **Deliver candidate completion notices to idle supervisors** — Candidate preparation success and failure notices now wake an idle supervisor instead of waiting for another turn or inbox poll.
