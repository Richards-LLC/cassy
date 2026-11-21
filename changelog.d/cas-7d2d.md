### Fixed — Commander

- **Queued commands survive daemon restarts** — Commander command receipts now
  persist locally by session, supervisor, device and command reference. Pending
  commands resume at the next native turn boundary after daemon restart or
  reexec. A durable claim prevents automatic replay of uncertain submissions;
  commands confirmed abandoned before typing show Not sent. A stale Queued
  status at session end remains uncertain until per-command evidence arrives.
