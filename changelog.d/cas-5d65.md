### Fixed — Factory

- **Declare peer merge holds explicitly** — Peer asks hold merges only when their first content line supplies `CAS_MERGE_HOLD` JSON with the task ID or PR number. Contextual mentions and legacy prose-only questions remain advisory; scoped holds retain answer correlation and audited overrides.
