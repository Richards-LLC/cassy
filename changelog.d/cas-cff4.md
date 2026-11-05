### Fixed — Factory

- **Requested changes reach the worker** — A supervisor's `request_changes` now sends the assigned worker one waking message with the decision and branch handling. Its output says which prompt carried it, or why no worker was told, instead of implying a handoff. Previously the task reopened silently and the worker sat idle until someone messaged it.
- **Stall alerts for sandboxed Claude workers** — Stall detection no longer counts a sandboxed worker's own harness process as a background job. Previously every Claude worker launched under a sandbox wrapper looked permanently busy, so neither the in-progress nor the assigned-but-unstarted stall alert could fire for it.
