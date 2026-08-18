# Slack draft — 2026-08-04 `cas claude <profile>` launches the factory again

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts per runtime rubric.

Follow-up to the same-day Claude account profiles post, which shipped `cas claude`
as a bare Claude Code launcher. That was not the intended surface.

## Post 1 — User

Live on main — **User**

Picking your second Claude account should start Cassy, not just open a chat window. `cas claude alt` now launches the full factory — supervisor and workers — signed in as your alt subscription.

- **Was → Now:** `cas claude alt` opened a plain Claude Code session with no Cassy around it → `cas claude alt` starts Cassy with a Claude supervisor running on the alt account, and every worker it spawns stays on that same account.
- **Was → Now:** choosing an account and choosing to run Cassy were two different commands you had to combine by hand with an environment variable → one command does both, and it prints which account directory it picked before anything starts.
- **Was → Now:** typing `cas claude` with no account name printed a list instead of starting anything → it starts the factory on whatever account you are already using, matching how `cas codex` and `cas grok` behave. The account list moved to `cas claude --list-profiles`.
- Still want just a chat window on a chosen account? `cas claude alt --bare` does exactly that, and passes your flags straight through.

## Post 2 — Dev

Live on main — **Dev**

`cas claude` is a factory provider shortcut with an account positional, not a `claude` exec wrapper.

- **Was → Now:** `cas claude <profile>` built a `Command::new("claude")` and `exec`'d it, so the factory never started → it exports the resolved `CLAUDE_CONFIG_DIR` into its own process, then delegates to the same `factory::execute` path as `cas codex` / `cas grok` with `supervisor_cli = "claude"` and the explicit flag set.
- The export happens in `cli::run` before `initialize_telemetry` spawns its background thread, alongside the existing resource-contention env bridge — `set_var` in a multi-threaded process is UB, and the daemon is produced by a bare `fork()`, so it inherits the value. Pane spawns snapshot `std::env::vars_os()` at `CommandBuilder::new`, which is how the supervisor lands on the chosen account; `spawn_workers` captures the same value as `requester_config_dir`, so workers follow without extra plumbing.
- **Was → Now:** an inherited `ANTHROPIC_API_KEY` was scrubbed only on the exec path, leaving the ambient-env route able to silently override subscription OAuth → the profile-selection path removes it too.
- Trailing arguments are parsed as `FactoryArgs` through `augment_args` + `from_arg_matches` rather than `#[command(flatten)]`, because `FactoryArgs` carries a subcommand that clap cannot disambiguate from the leading profile positional. Unknown flags exit through clap's own error path instead of being swallowed.
- `--list-profiles` keeps the account/login/active listing; `--bare` keeps the previous exec-Claude-Code behavior with argument passthrough.
