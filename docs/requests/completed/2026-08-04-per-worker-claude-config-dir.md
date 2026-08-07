> **Disposition (2026-08-07, cas-ab75):** DELIVERED — filed as [#72](https://github.com/pippenz/cas/issues/72) (closed completed; duplicate filing [#77](https://github.com/pippenz/cas/issues/77) closed as duplicate). Verified on `main`: epic cas-adae merged as `d27aa3ec` — `3c2c7ac5` "feat(cas-740c): support per-spawn Claude config dirs" plus migration `m216_spawn_queue_add_requester_config_dir`; `spawn_workers` accepts `config_dir`, explicit value wins over the supervisor's inherited `CLAUDE_CONFIG_DIR`, and an inherited `ANTHROPIC_API_KEY` is stripped so the selected OAuth account is used. Archived.

# Request: per-worker auth isolation via CLAUDE_CONFIG_DIR on spawn_workers

**Date:** 2026-08-04
**From:** wise-lynx-41 (Penguinz factory supervisor), on behalf of pippenz

## Problem

The operator has two Claude subscriptions and wants CAS workers to be able to
use a different account than the interactive/supervisor session. Today all
`claude` CLI processes on the machine share `~/.claude/.credentials.json`, so
`/login` in any one session switches the account for every session and every
spawned worker.

Claude Code supports `CLAUDE_CONFIG_DIR`: pointing it at a different directory
redirects `.credentials.json` (plus `.claude.json`, history, etc.) for that
process. Two config dirs, each `/login`-ed once with a different account, give
two isolated logins selected purely by environment variable.

## Ask

Add per-spawn environment control to `coordination action=spawn_workers`:

- Minimal: a `config_dir` (or generic `env`) parameter applied to the spawned
  worker processes, e.g. `spawn_workers count=2 cli=claude config_dir=~/.claude-alt`.
- Nice-to-have: a named-profile map in `.cas/config.toml`
  (`[worker_profiles.alt] claude_config_dir = "~/.claude-alt"`) and
  `spawn_workers profile=alt`, so account choice is declarative and mixed
  fleets (some workers on sub A, some on sub B) are possible.

## Interim workaround (works today, all-or-nothing)

Launch the whole factory/daemon shell with `CLAUDE_CONFIG_DIR=~/.claude-alt`
so every spawned worker inherits the alt account, while the operator's own
interactive terminals use the default `~/.claude`. No per-worker mixing.

## Caveats worth encoding

- `CLAUDE_CONFIG_DIR` is community-known but not fully documented upstream
  (anthropics/claude-code#3833, #25762); project-local `.claude/` dirs are
  still created in worktrees regardless.
- `ANTHROPIC_API_KEY` in the worker env overrides subscription OAuth entirely
  — the spawner should either pass it through deliberately or scrub it.
