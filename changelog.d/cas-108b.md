### Fixed — MCP task recovery

- **Accurate slow task responses** — A committed task start now reports accepted assignment and points to saved start context, while close responses retain their close outcome. Slow read-only task calls return explicit pagination guidance and an executable bounded request instead of a bare deadline error.
