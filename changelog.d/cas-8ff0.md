### Fixed — Factory

- **Codex supervisor responsiveness** — Deliver pending mail at turn start within the existing 10-row / 5,000-character budget, retaining overflow for the next turn, and surface new mail after Cassy MCP calls. Resolve supervisor rollouts for idle wakes and save a handoff before restarting a supervisor under context pressure. The default context cap is now 300,000 input tokens, bounded by 80% of the reported window.
