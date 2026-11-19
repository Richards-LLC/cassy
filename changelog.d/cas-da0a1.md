### Fixed — Factory

- **A Codex worker's first wake waits for its Cassy tools** — A Codex factory worker could receive its assignment and supervisor wakes while its Cassy MCP server had not yet listed `task` and `coordination`, so the worker had no tools to act with (GH #2128). The worker's server now records when it first lists its tools, and the daemon holds that worker's wake (visible as "gated not ready" in `message_status`, with a pointer to `cas factory doctor`) until a live server has done so. No timer is involved. Supervisors, broadcasts and Claude workers are unchanged.
