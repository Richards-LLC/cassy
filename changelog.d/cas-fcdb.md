### Fixed — Builtin skills

- **One `/finish` in each harness picker** — Was: Claude, Codex and Grok listed
  `/finish` twice, once from the user-level install and once from the project's
  copy, and the old personal seed (Claude `commands/finish.md`, Codex
  `prompts/finish.md`, OpenCode `commands/finish.md`, the Grok personal skill)
  sat beside the builtin. Now: finish installs once at user level, project sync
  removes its earlier managed copy, and `cas update` removes the exact old seed
  from every harness. Edited copies are left alone.
- **Shorter `/finish`** — Was: a long checklist that repeated steps Cassy already
  enforces (the exit check, handoff supersede, gc report) and polled worker
  status by hand. Now: a short list of the steps that need you, with worker
  shutdown relying on Cassy's own refusal and remedy.
