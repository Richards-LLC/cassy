### Fixed — Factory daemon

- **Message selection and the spawn poll no longer hold the store writer while they read** — Was:
  each prompt-queue selection and each spawn-queue poll ran whole on the daemon's
  single store writer, including the roster, worker-death scan, notice checks and
  the per-message director and git reads, holding it up to 4.1 s and 3.0 s on a
  busy factory, so Commander sends and other writes waited behind them. Now: both
  run on their own select lane and take the writer only for their writes; a poll
  with nothing for this session never takes it.
