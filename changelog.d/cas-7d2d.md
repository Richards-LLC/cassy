### Fixed — Commander

- **Queued commands survive daemon restarts** — Commander command receipts now
  persist locally by session, supervisor, device and command reference. Pending
  commands resume at the next native turn boundary after daemon restart or
  reexec. A durable claim prevents automatic replay of uncertain submissions;
  commands confirmed abandoned before typing show Not sent. A stale Queued
  status at session end remains uncertain until per-command evidence arrives.
  Every session teardown records queued commands as Not sent without letting a
  receipt-store failure skip the rest of teardown. A queued command left by an
  earlier daemon under the same session and supervisor names is never typed
  into a new one, and an unreadable receipt no longer blocks restoring the rest.
