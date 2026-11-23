### Fixed — Factory

- **Reconnected MCP versions stay known** — Was: when an MCP server started while other factory processes were writing, its startup version record could be lost. `worker_status` then showed the server's version as unknown, a heartbeat did not fix it, and Commander's Reload Cassy refused because no live PID and version were available. This was seen after `/mcp reconnect` (GH #2183). Now: the record waits for the other writer instead of being dropped, so the reconnected server's PID and version are reported.
