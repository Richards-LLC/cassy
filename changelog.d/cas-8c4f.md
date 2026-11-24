### Added — Commander

- **Reload Cassy on Codex supervisors** — Was: Reload Cassy was refused for
  Codex. Now: after a one-line confirmation it restarts the Codex supervisor on
  the installed build while workers keep running, and shows the same proof.
- **Loaded-build badge** — Was: nothing showed which Cassy build a supervisor
  had loaded after a reload. Now: the supervisor card, the conversation header
  and the factory status bar show "cassy 4.52.0 · loaded 5:44 PM ET" from the
  live MCP process until the next reload.
- **`/reload-cassy`** — Was: only `/mcp` ran Reload Cassy. Now: `/reload-cassy`
  runs the same command, with the same Codex confirmation.

### Changed — Commander

- **Reload Cassy waits instead of refusing** — Was: a busy supervisor refused
  Reload Cassy. Now: it shows a small waiting animation and reloads at the next
  quiet moment, without interrupting a running tool call. A supervisor already
  on the installed build gets its badge and nothing is sent.
- **Reload proof checks the installed build** — Was: any replacement MCP process
  counted as reloaded. Now: success needs the live version to equal
  `cas --version`; a mismatch says which version runs and what to do next.

### Fixed — Commander

- **No false reload failure under a brief database hold** — Was: a reload that
  worked could show "shared SQLite connection held" when another thread held
  the database for a few milliseconds. Now: receipt writes get their own short
  wait budget, and a contended receipt copy is retried instead of reported as a
  failure.
