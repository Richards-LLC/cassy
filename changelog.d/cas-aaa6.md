### Fixed — Commander

- **Received supervisor replies are no longer reported abandoned** — Was: a device receipt that arrived after the delivery deadline, or a transport receipt for a reply sent to any Commander device, left the reply marked abandoned. Now: the authenticated receipt settles it as delivered.
- **Pane replies keep reaching Commander after an MCP reconnect** — Was: the old MCP's exit deleted the supervisor row, so the new MCP resumed the pre-`/clear` transcript and every pane reply went unmirrored. Now: the reconnect resumes the session the row was last bound to.
