### Fixed — Release preview

- **Linked worktree project identity** — Release previews in linked worktrees without local Cassy configuration inherit the main checkout's canonical project identity through Git's common directory, avoiding false foreign-scope holds. An explicit local project pin keeps precedence.
