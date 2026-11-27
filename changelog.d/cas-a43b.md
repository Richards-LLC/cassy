### Fixed — Factory

- **Worker resumes survive a sibling MCP connection closing** — A Codex worker no longer loses readiness when a second connection lists Cassy's tools and exits. A new worker launch still waits for its own catalog listing.
