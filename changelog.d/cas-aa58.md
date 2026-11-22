### Fixed — Skills

- **`skill action=show` serves the caller worktree's skill** — Was: in a
  worker worktree with a newer `.claude/skills/<name>/SKILL.md`, `skill
  action=show` returned the database copy or the main checkout's older body.
  Now: the caller's own checkout is read first, then the database, the main
  checkout and the builtin catalog, so a worker sees the skill it is editing.
