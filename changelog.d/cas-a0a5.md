### Fixed — Factory

- **Codex pre-merge verifiers can reach Cassy** — The daemon-owned verifier on the Codex recipe now pre-approves Cassy's own MCP server, so its first tool call is no longer refused under `approval: never`. A merge retry after a failed verifier attempt reports the scheduled replacement instead of asking for a verdict that nobody was producing, and the factory daemon logs a refused verifier launch.
