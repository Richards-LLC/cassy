### Fixed — Builtin skills

- **Your own `/finish` command is kept** — Was: `cas update` deleted a Claude
  `commands/finish.md` whose bytes matched the old personal seed, then
  installed the builtin finish skill in its place. Now: any
  `commands/finish.md` is yours and is never removed, and update removes an
  installed builtin finish skill beside it so the command stays callable.
- **One `/finish` in the Claude picker** — Was: Cassy-initialised projects
  also got `.claude/skills/finish`, so the picker listed `/finish` twice. Now:
  finish lives only at user level, and project sync removes the old managed
  project copy.
