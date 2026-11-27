### Fixed — CLI

- **Managed-file sync leaves shared branches untouched** — Updates now defer managed project files on default and integration branches. Sync on a repair branch and submit the reviewed changes by PR instead of leaving a local-only commit on the merge target.
