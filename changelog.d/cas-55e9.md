### Fixed — Factory daemon

- **A slow delivery read no longer backs up the daemon** — Was: when the
  supervisor delivery read ran longer than its 30-second wait, every later
  pass queued another one behind it, about twenty in ten minutes, and
  messages to the supervisor waited behind them. Now: a new request replaces
  the one already waiting, so at most one waits behind a running read.
- **In-place upgrade works on a busy daemon** — Was: `cas factory reexec`
  refused whenever any background job was queued or running, so a
  backlogged daemon could never load the fix. Now: refresh jobs, which rerun
  after the reexec, no longer hold it; it still waits for delivery and
  durable work.
