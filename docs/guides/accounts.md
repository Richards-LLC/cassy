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

A symlinked account directory represents its target, whether that target is
inside or outside your home directory. Linking and `cas update --user` follow
that target. When several account aliases reach the same canonical path,
mutation discovery processes the target once; the registry and launchers keep
all aliases available for selection.

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

## List subscriptions

```bash
cas accounts list
cas accounts list --json
```

The grouped list names Claude and Codex subscriptions by their login email,
with the folder alias, plan, login state, usage and reset times. `main`, `alt`
and email folder names remain valid arguments to `cas claude`, `cas codex`
and worker `config_dir` settings. Display names do not change those aliases.

`duplicate_of` identifies a second folder holding the same provider account
and organization/workspace ID. Different subscriptions sharing an email stay
separate; email is the fallback identity only when no provider ID is available.
`never_logged_in` means no stored credential or previous-login evidence was
found. `stale_login` requires a rejected credential; a failed probe stays
unknown. Provider work shares a four-second deadline; missing usage is unknown,
never zero. JSON reset timestamps are absolute UTC RFC3339 values.

The shared API is `account_registry::{Account, AccountFlags, LoginState,
UsageWindow, collect, discover_with}`. `Account.name` is the launch alias and
`Account.folder` is the stable directory key. Selection code must check login,
usage windows and flags independently; `windows: []` is unknown capacity.

### Limits JSON reset timestamps

`cas limits --json` now returns each window's `reset_at` as an RFC3339 UTC
instant (for example `2026-10-27T17:00:00+00:00`), replacing the old localized
`YYYY-MM-DD HH:MM ZONE` display string. Consumers should parse the datetime,
rather than split or compare localized text. `cas limits` and picker summaries
still render local time for people. `cas accounts list --json` uses the plural
field `resets_at`, also an absolute UTC instant; unknown resets are `null`.

### Launch by email or folder alias

Claude and Codex try an exact existing folder alias first, then a unique login
email. If several folders share that email, choose one of the aliases named in
the error. A typo refuses before launch or spawn, suggests the closest alias
and names the explicit login command; launching never creates an unknown
account folder. Worker `config_dir` accepts the same aliases/emails as well as
existing directory paths.

Registry `identity` is an object with `account_id`, `workspace_id` (the Claude
organization) and `email` (populated only for a fallback without provider IDs).
Codex folders sharing that identity use the newest completed usage snapshot.
A window whose reset has passed says `reset, usage unknown until next session`;
its JSON percentage is `null` until a new transcript reports usage. Canonical
accounts precede the duplicates that point to them.
