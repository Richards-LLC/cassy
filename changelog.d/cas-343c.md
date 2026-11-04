### Fixed — Factory

- **Batch verdicts acknowledge committed receipts** — A slow delivery close no longer turns a recorded batch verdict into an unknown timeout. Closing continues through its existing gates, and task notes retain each result, including failures.
