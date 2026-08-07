# Slack draft — 2026-08-06 v2.42.0 (codex trust + wake-ups, blocked_by, epic lifecycle, sweep skill)

Channel: #cas-internal (C0B44GUKDK2). Two top-level posts per runtime rubric.

**Status:** POSTED 2026-08-06 20:50:18 EDT to `#cas-internal` (`C0B44GUKDK2`).
- User top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063818043499
- User reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063826224449
- Dev top-level: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063829935809
- Dev reply: https://petra-stella.slack.com/archives/C0B44GUKDK2/p1786063841059909

## Post 1 — User

Live on main — **User**

Codex crews stopped needing a babysitter: new projects no longer hang silently at startup, handed-off work actually starts, and setting a blocker actually blocks.

- **Was → Now:** launching Codex workers in a brand-new project directory hung forever on a hidden "trust this folder?" prompt, surfacing only as a generic 60-second timeout → the directory is trusted automatically before launch, and if registration still times out the error names the real cause instead of shrugging.
- **Was → Now:** handing a task to a Codex worker often did nothing — the work sat untouched while "worker is idle" notices insisted there was nothing to do → assignments now reliably wake Codex workers, and idle notices carry the timestamp of the state they actually saw, so a stale claim is identifiable as stale.
- **Was → Now:** adding a blocker to an existing task silently did nothing and reported "No changes specified", letting work start on inputs that weren't ready → blockers added after creation now take effect exactly like blockers set at creation.
- **Was → Now:** finishing a project phase and then needing one small follow-up forced you to invent a whole ceremonial phase around a single task → a worker can now be launched for a concrete task with nothing else open.
- **Was → Now:** starting a new phase while an old one was checked out could quietly branch from a weeks-old baseline, hiding everything done since → the divergence is now detected and called out instead of silently stranding prior work.
- **New:** the recurring GitHub-issues sweep (dedupe double-filings, verify fixed claims, file follow-up work, unblock waiting tasks) is now a built-in skill instead of an improvised prompt that named a skill which didn't exist.

## Post 2 — Dev

Live on main — **Dev**

Codex PTY launch gained a pre-spawn trust writer and durable assignment wake-ups; `task update` finally honours `blocked_by`; epic-branch creation checks its base against HEAD; the GitHub-issues sweep ships as a builtin skill for all three harnesses.

- **Was → Now:** Codex workers launched into a workdir absent from `~/.codex/config.toml [projects]` parked pre-MCP-init on the interactive trust prompt (no session JSONL, no `cas serve` child, `stage=register` timeout after 60s) → a new `cas-pty` codex-trust module writes `trust_level = "trusted"` entries for worker and supervisor workdirs before launch, hardened against config corruption, and the register-timeout diagnostic names the untrusted-dir cause for `cli=codex`.
- **Was → Now:** assignee changes never reached the Codex injected-turn path (Claude workers woke via the harness inbox; Codex workers required a manual message) → assignment wake-ups are delivered durably on the Codex path, and director idle notices are stamped with the read instant of their snapshot.
- **Was → Now:** `task action=update` dropped `blocked_by` silently ("No changes specified", even alongside recognised fields) → blockers are pre-validated and dependency rows created on update, with gated status re-armed, matching `create` semantics.
- **Was → Now:** `spawn_workers` hard-required an active epic even with a concrete open `task_id` → supplying an open `task_id` authorizes the spawn post-epic-close; the graph no longer accretes single-child wrapper epics.
- **Was → Now:** epic-branch creation based on `main` unconditionally, even with HEAD dozens of commits ahead on a prior epic branch → creation compares intended base vs HEAD and surfaces the divergence with a commit count.
- **Measured and rejected:** dev-profile `split-debuginfo` — with mold and `debug = 1` already in place it buys no relink time (~5s either way), no cold-build time, and net **+23 MB** disk at measured scale; `packed` emitted 818 MB of per-executable `.dwp` and is ruled out. Profile left alone; full numbers on the issue.
- Lands GitHub issues #94–#100 via `Fixes` trailers on the merge; the debug-profile issue closed with the measurement write-up.
