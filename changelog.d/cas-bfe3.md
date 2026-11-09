### Fixed — Commander

- **Supervisor answers keep reaching Commander after an MCP reconnect** — Reconnecting a supervisor's Cassy tools after a `/clear` no longer points it back at its pre-clear transcript. Before, every later pane answer and status update was silently missing from Commander until the next session start.
