# Slack draft — 2026-08-04 Claude account profiles (per-spawn account control)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts per runtime rubric.

## Post 1 — User

Live on main — **User**

One machine, two Claude subscriptions, zero juggling. It used to be that every Claude session and every factory worker on the machine shared a single login — logging into a different account anywhere silently switched it everywhere, and nothing told you which account a worker was actually burning. Now you pick the account per launch, and CAS says out loud which one it used.

- **Was → Now:** shell-alias gymnastics and silent account mix-ups → `cas claude alt` opens Claude Code on your alt subscription from any shell, no aliases or rc-file setup needed.
- Plain `cas claude` lists every account profile on the machine, shows which are logged in, and which one is active right now.
- Workers spawned from a session running on the alt account now stay on the alt account instead of silently falling back to whatever the background daemon was started with.
- Every worker launch now records which Claude account directory it used and why — a wrong-account launch can't hide anymore.
- Heads-up: `cas claude` previously launched the factory with a Claude supervisor. That spelling moved to `cas factory --supervisor-cli claude` (add `--default` to persist it).

## Post 2 — Dev

Live on main — **Dev**

`CLAUDE_CONFIG_DIR` goes from invisible ambient state to a first-class, logged spawn parameter.

- **Was → Now:** worker account selection was pure env inheritance from the daemon process (set at daemon start, unchangeable, unlogged) → a resolved config dir with explicit precedence: `spawn_workers config_dir=…` > the requesting session's own `CLAUDE_CONFIG_DIR` (captured at enqueue time, persisted on the spawn-queue row via a new column + migration) > classic inheritance, byte-for-byte unchanged when neither is set.
- `config_dir` is tilde-expanded and Claude-only; Codex/Grok spawns ignore it with a warning instead of failing.
- Explicitly selecting an account scrubs any inherited `ANTHROPIC_API_KEY` from that spawn, since an API key would override subscription OAuth and defeat the selection.
- Every Claude worker spawn emits a tracing line with the effective account dir and its source (`explicit param` / `supervisor session` / `host env` / `default`).
- New `cas claude <profile>` subcommand execs Claude Code with the profile's config dir (`main` → `~/.claude`, anything else → `~/.claude-<name>`), forwards trailing args, and lists profiles + login state when run bare. It replaces the old factory provider-shortcut; that behavior lives on as `cas factory --supervisor-cli claude [--default]`.
