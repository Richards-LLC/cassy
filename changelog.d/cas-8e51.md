### Added — Scorecard

- **What each task cost, by model, effort and harness** — Was: the release
  report totalled tokens by who woke the agent, so Codex and Claude spend per
  task had to be mined from raw transcripts. Now: each turn records its
  harness, model, effort, cached input and the task a worker held, and the
  report adds spend per task and per harness, model and effort, with cached
  input kept apart.
- **Sensor health at the top of the report** — Was: a sensor with no samples
  or a broken input printed a quiet zero. Now: every sensor is listed as OK,
  EMPTY or BROKEN with its reason first, including the runner-loss, estimate,
  feedback, cross-machine park, unknown-wake and over-30-day duration cases.
- **Unknown wakes name their real cause** — Was: wakes from another project
  were all blamed on a missing local agent. Now: they are reported against the
  emitting project that records no notice kind, so only this project's own
  gaps point at its agents.
- **`cas scorecard` is the report's name** — Was: `cas ledger report`. Now:
  `cas scorecard --release vX.Y.Z`; `cas ledger report` still works.
