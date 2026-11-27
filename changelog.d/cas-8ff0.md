### Fixed — Factory

- **Codex supervisor responsiveness** — Deliver pending mail together at turn start and after Cassy MCP calls, resolve supervisor rollouts for idle wakes, and save a handoff before restarting a supervisor under context pressure. The default context cap is now 300,000 input tokens, bounded by 80% of the reported window.
