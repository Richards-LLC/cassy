# One shared base folder for every Claude and Codex account

Each named account directory keeps only its login and runtime state. Every
other shared item is a symlink into one base folder per provider, so a command,
skill, setting or project memory added on one account is there on all of them.

| Provider | Base folder | Shared items | Kept per account |
| --- | --- | --- | --- |
| Claude | `~/.cas/claude-base` | agents, commands, skills, plugins, hooks, workflows, output-styles, settings.json, settings.local.json, CLAUDE.md, projects | `.credentials.json`, `.claude.json`, sessions, session-env, caches, locks |
| Codex | `~/.cas/codex-base` | AGENTS.md, agents, hooks.json, config.toml, plugins, skills, memories | `auth.json`, `installation_id`, sqlite state, logs, sessions, locks |

`config.toml` is shared: Cassy injects its MCP server per spawn, so the file
holds no per-account key.

## Link once

Preview, then apply. Run it yourself, with no Claude or Codex session open on
these accounts:

```bash
cas accounts link --dry-run
cas accounts link
```

The dry run lists every merge, conflict, link and directory change. The apply:

1. Moves every item it replaces, and every directory it retires, into
   `~/.cas/accounts-backup/<time>-<provider>/`.
2. Copies each account's shared files into the base. A file that differs from
   the base copy is kept beside it as `<name>.from-<account>`, and listed.
3. Retires `~/.claude`, `~/.claude-alt` and `~/.codex` as accounts. A login
   that duplicates a named account moves into the backup; a unique login is
   renamed to its named directory (`~/.claude-<email>`). Misspelled
   directories with no login, or with a login another named directory
   already holds, move into the backup too.
4. Links each account's shared items to the base.

It prints the backup and the undo command (`bash <backup>/restore.sh`). A
second run changes nothing. Run the same command on each machine.

New accounts created with `cas claude` or `cas codex` link to the base
automatically. `cas doctor` reports an account whose shared items no longer
link to the base (`account links`); `cas accounts drift` lists them.

## See which account a session uses

- The Claude status line (`cas statusline`, already in the shared settings)
  starts with the account email.
- `cas claude` and `cas codex` print the account at launch.
- The factory status bar shows the focused pane's account, Claude or Codex.

## Switch after a usage limit

```bash
cas claude --list-profiles   # marks an account whose limit is used up, with its reset time
cas claude <profile>         # switch in one step
cas claude --bare <profile> --continue   # resume the last session in this folder
```

The picker shows the same usage marks. Sessions resume across accounts
because `projects/` lives in the base. `cas codex` works the same way.

## Choose the Codex account for a factory worker

Factory capability checks inspect registered Codex homes when `CODEX_HOME` is
unset. An authenticated named account keeps Codex lanes available after the
legacy `~/.codex` is retired. An explicit `CODEX_HOME` limits the check to that
account.

A Codex worker uses its authenticated default `~/.codex` when no account is
selected. If that home is absent or logged out, pass a registered account's home
as `config_dir`, or set the worker `config_dir` in the project's
`.cas/config.toml` (`[factory.defaults]` or `[[factory.workers]]`). The project
setting also applies to hub and Commander spawns. A requesting supervisor's
`CODEX_HOME` remains an explicit selection.

When the implicit default is unavailable, spawn refuses before creating a
worktree or assigning a task and lists the registered homes confirmed logged
in. Cassy does not select a different account automatically.
