### Fixed — Factory

- **Supervisor handoff ownership** — Saving or restoring a supervisor handoff now stays within its machine and logical supervisor lineage, including explicit local restarts. A peer supervisor's newer handoff cannot replace local context or inject its production approvals. Unowned legacy handoffs remain available through memory tools.
