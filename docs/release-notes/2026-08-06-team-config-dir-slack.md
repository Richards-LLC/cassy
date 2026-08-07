# Release notes — team files follow the active Claude account (main, 165312f7)

**Channel:** `#cas-internal` (`C0B44GUKDK2`)
**Status:** POSTED 2026-08-06 20:51:16 EDT to `#cas-internal` (`C0B44GUKDK2`).
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063876122129
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063881524019
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063885048959
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063892299609

Post order: user top-level → capture `ts` → user reply → dev top-level → capture `ts` → dev reply.

=== MESSAGE 1 — User thread, top-level ===
Live on production — **User** — Team sessions launched on your second Claude account now actually find their own team.

=== MESSAGE 2 — User thread, reply (to MESSAGE 1 ts) ===
**Was →** if you started a team session under an alternate Claude account, the team's files were written into your main account's folder instead. Claude then went looking for them in the account it was actually running as, found nothing, and the session came up as a team that could not see itself — messages between members went nowhere and the shared task list read as missing. From the outside it looked like the whole setup was broken, and the usual fixes didn't help because the files were fine, just in the wrong place.
**Now →** the team's folder is created inside whichever account the session is running as, so everything lines up on first launch. Single-account setups are untouched and behave exactly as before.

=== MESSAGE 3 — Dev thread, top-level ===
Live on production — **Dev** — The team directory and every `--settings` path it hands to `claude` now resolve from the active `CLAUDE_CONFIG_DIR` instead of a hardcoded `~/.claude`.

=== MESSAGE 4 — Dev thread, reply (to MESSAGE 3 ts) ===
**Was →** `TeamsManager` built four paths — the team directory, the inbox directory, and the eagerly pre-written supervisor and per-member settings files — from a literal `home/.claude/teams`. Launching under an alternate account exports `CLAUDE_CONFIG_DIR=~/.claude-alt` into the process before anything spawns, and the daemon inherits it, so Claude Code read `~/.claude-alt/teams/<team>/config.json` while those files had been written to `~/.claude/teams/<team>/`. Every path derived from the config dir — inbox writes, member discovery, task-list lookup — pointed at a directory that did not exist, and the symptom read as missing hooks rather than a path mismatch.
**Now →** all four call sites go through a single `teams_root_dir()` backed by a pure `claude_config_dir_from(home, env)`: an explicit `CLAUDE_CONFIG_DIR` wins, `~`-prefixed and relative values expand against home, blank values are ignored, and the fallback stays `~/.claude` — the same resolution semantics already used for hook coverage. Regression tests assert that an alternate config dir moves the team directory, the inbox directory, and the pre-written settings files under it while writing nothing to the default dir, plus a default-dir case pinning the original layout.
