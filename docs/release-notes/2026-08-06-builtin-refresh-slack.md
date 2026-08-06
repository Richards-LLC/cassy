# Release notes — builtin refresh (main, v2.44.0)

**Channel:** `#cas-internal` (`C0B44GUKDK2`)
**Status:** DRAFT — not yet posted. Slack transport unavailable on 2026-08-06
(claude.ai Slack connector unauthorized on the posting profile; codex_apps path
blocked until the Codex quota reset on 2026-08-08 10:07am). Post as soon as a
transport is available.

Post order: user top-level → capture `ts` → user reply → dev top-level → capture `ts` → dev reply.

=== MESSAGE 1 — User thread, top-level ===
Live on production — **User** — Finished work stops getting bounced for the words used to describe it.

=== MESSAGE 2 — User thread, reply (to MESSAGE 1 ts) ===
**Was →** when a job was reviewed for completion, one of the assistant flavors screened the summary for suspicious-sounding words — "pending", "partial", "remaining items". A summary that said the job was done but noted something a *different* team still owed would trip the filter and get rejected, so genuinely finished work bounced back and had to be re-explained. The other flavors had stopped doing this months earlier, so whether your work passed depended on which assistant happened to review it.
**Now →** the review reads the job's own definition of done and rejects only when the summary describes something that definition actually requires. Work that mentions somebody else's outstanding item goes through. Every flavor judges the same way, and the built-in guides now describe the real system — how to pick which account a helper runs under, how long-running servers stay alive, and what a completed review has to show for itself.

=== MESSAGE 3 — Dev thread, top-level ===
Live on production — **Dev** — Codex task-verifier now runs the AC-based close-reason check instead of the retired keyword blacklist, and a normalized three-way test pins all builtin guides across flavors.

=== MESSAGE 4 — Dev thread, reply (to MESSAGE 3 ts) ===
**Was →** the Codex copy of `task-verifier.md` had not moved since its creation and missed two upstream commits. It still rejected close reasons by string match against `remaining items` / `pending` / `partial implementation` — the exact heuristic dropped after a false positive on a close reason that mentioned another team's outstanding dependency — and its check recipes were Rust-only (`ast-grep --lang rust`, `.rs` fixtures). The Codex supervisor checklist was four months stale: no binary-freshness preflight, no codemap check, no `epic_status` merge gate, no session-hygiene report. Supervisor and worker guides documented behavior the code no longer had: `spawn_workers` was missing `task_id` and `config_dir`, the server-registry and hold/release actions were absent from lists that claimed completeness, raw `git merge --no-ff` was taught as the primary merge path over `worktree_merge`, and the mandatory `execution` block in the close envelope was undocumented. Two deprecation stubs shipped unadapted Claude copies pointing at `.claude/` paths that do not exist on their flavor. Nothing tested cross-flavor equivalence, so each of these could sit indefinitely.
**Now →** Codex verifiers match Claude and Grok: reject only when the close reason describes work the task's own acceptance criteria require, with recipes covering TypeScript and Python alongside Rust. The Codex checklist carries the binary-freshness preflight, codemap freshness check, `gc_report` hygiene triage, and the bypass-immune `epic_status` merge gate — in `mcp__cs__` spelling, with the no-hooks adaptations kept (codemap and `gc_report` are explicit every-session steps because Codex has no SessionStart banner to fire them). Guides are corrected against their dispatch sites, including the `force` / `allow_trunk` / `cleanup` flags that are independent and get conflated. `builtin_flavor_drift_test.rs` now normalizes the intended per-flavor differences — tool prefix, path roots, catalog constant — and asserts byte equality on the remainder, failing the build on anything else. Also in this cut: the SessionStart payload is assembled under a 9KB aggregate budget with deterministic degradation, and team directories plus hook configure/cleanup resolve from the active `CLAUDE_CONFIG_DIR` rather than a hardcoded `~/.claude`.
