### Fixed — Artifact sharing

- **Fetch proof on a cold peer** — Artifact fetch now resolves its configured project before account-wide discovery and uses Cloud's task endpoint correctly. Committed proof remains fetchable when the receiving machine lacks local metadata and the account has many projects.
