### Fixed — CLI

- **Managed-file sync leaves shared branches untouched** — Updates now defer managed project files on default and integration branches. Sync from a repair worktree using the shared project's settings, then submit the reviewed changes by PR instead of leaving a local-only commit on the merge target.
