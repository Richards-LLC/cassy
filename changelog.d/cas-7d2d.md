### Fixed — Commander

- **Queued commands survive daemon restarts** — Commander command receipts now
  persist locally by session, supervisor, device and command reference. Pending
  commands resume at the next native turn boundary after daemon restart or
  reexec. A durable claim prevents automatic replay of uncertain submissions;
  commands abandoned before typing show Not sent when the supervisor ends.
