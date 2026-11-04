### Fixed — Worker spawn admission

- **Choose free worker branch names** — Isolated MCP spawns reserve generated
  names against local and origin factory refs before queueing work. Explicit
  collisions give a clear refusal before task binding or worktree creation.
- **Explain Codex account refusals** — Spawn errors name a missing default home,
  a logged-out account or a mismatched configuration directory and retain the
  account-selection diagnostics that explain why a named login was not chosen.
