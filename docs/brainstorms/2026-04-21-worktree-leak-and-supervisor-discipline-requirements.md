---
date: 2026-04-21
topic: worktree-leak-and-supervisor-discipline
---

# Worktree Leak Reclaim + Supervisor Agent-Path Discipline

## Problem Frame

Two linked failure modes affect every Cassy user running multi-repo factories: disk bloat from abandoned worktrees, and silent drift between "what Cassy thinks is running" and "what is actually running."

1. **Factory worktrees don't get cleaned up.** In the reporting user's environment, a snapshot across ten repos found 162 orphan worktrees under `.cas/worktrees/` (plus nine prunable `/tmp/cas-*-wt`); this pattern reproduces anywhere Cassy factories are used heavily. Three code-level causes:
   - `shutdown_worker` in `cas-cli/src/ui/factory/app/render_and_ops/epic_workers.rs:333-334` deliberately skips removal with the comment "We don't delete clones by default as they may have uncommitted work."
   - `mark_worker_crashed` preserves the dir for potential respawn.
   - The daemon's `cleanup_orphaned_worktrees` in `cas-cli/src/daemon/maintenance.rs:272` only iterates rows in `worktree_store.list_active()`; it never scans the filesystem, so unregistered worktrees (pre-DB, crashed-before-registration, or from the Agent path below) are invisible to it.
2. **Supervisors occasionally spawn raw `Agent(isolation: "worktree")` subagents** instead of `mcp__cas__coordination spawn_workers`. The resulting dirs live under `.claude/worktrees/agent-*` — outside Cassy's factory accounting entirely (no task rows, no agent rows, no merge pipeline). A supervisor was observed narrowly catching itself mid-spawn after the `cas-supervisor` skill loaded; under pressure, the default tool wins. This affects any Cassy user whose supervisors have access to the `Agent` tool.

Impact: disk exhaustion, plus silent erosion of Cassy's authoritative view of running work.

## Requirements

**Cleanup Defaults (shutdown path)**
- R1. When a worker's task transitions to Closed and its worktree has no uncommitted/untracked files, Cassy must automatically remove the worktree directory and delete the branch (if safe). This replaces the "keep by default" stance in `shutdown_worker` and `mark_worker_crashed`.
- R2. When the tree is dirty at close time, cleanup must skip with a loud, actionable warning naming the worktree path and the dirty-file summary. The dirty worktree remains until the user resolves it or the daemon reaper triggers (R5).

**Daemon FS-Driven Reaper (ongoing)**
- R3. The daemon's orphan sweep must be filesystem-driven in addition to DB-driven: it walks `.cas/worktrees/*` (and `.claude/worktrees/agent-*`) directly, reconciles each dir against git + the Cassy DB, and reclaims anything it determines is orphaned.
- R4. The sweep must operate across all Cassy-known repos on the host, not just the daemon's current repo. The specific topology (cross-repo daemon vs per-repo daemon + global sweeper vs host-level cron) is deferred to planning.
- R5. An agent is considered "abandoned" after 24 hours with no heartbeat/activity. Worktrees attached to abandoned agents are eligible for reclaim.
- R6. When a reclaim candidate has uncommitted/untracked work, the daemon must write a salvage patch to `.cas/salvage/<YYYY-MM-DD-HHMMSS>-<worker-name>.patch` (git diff + untracked file contents) before removing the worktree. No user-visible work may be silently destroyed.
- R7. TTL threshold (currently 24h) must be configurable via `cas config` with a documented default.

**One-Shot Backlog Sweep (existing leaks on any user's host)**
- R8. Ship a CLI command that performs a safe one-shot sweep: for each leaked worktree, remove only if (a) its branch is merged into its parent/epic branch AND (b) the tree has no uncommitted/untracked files. Anything that fails either check is listed in a report and left alone.
- R9. The command must discover the set of Cassy-tracked repos on the current host from Cassy's own registry (not a hardcoded path or env-specific assumption) and sweep all of them in one invocation, with a dry-run mode and an explicit opt-in for multi-repo scope.
- R10. Output must include counts per repo, space reclaimed, and an itemized list of skipped worktrees with the reason (dirty, unmerged, dirty+unmerged).
- R15. The command must work on any host regardless of directory layout — no assumption that repos live under a common parent directory. Discovery is driven by Cassy's own records of repos it has touched.

**Supervisor Agent-Path Block**
- R11. Supervisors must be prevented at harness level from spawning subagents with `isolation: "worktree"`. Preferred mechanism: PreToolUse hook on the `Agent` tool that inspects the args and blocks when isolation is set to worktree, emitting an error that directs the supervisor to `mcp__cas__coordination spawn_workers`.
- R12. Non-isolation `Agent` calls (Explore, code review personas, research) must continue to work unmodified. Only the worktree-isolation variant is blocked.
- R13. The daemon reaper (R3) must also detect and clean `.claude/worktrees/agent-*` directories that slip through R11, using the same salvage-then-remove policy as R6.
- R14. The `cas-supervisor` skill and supervisor system prompt must be tightened to forbid raw `Agent(isolation: ...)` spawning, with a named rule the reviewer can cite.

## Success Criteria

- After one full factory session cycle on any Cassy host: zero leftover worktrees in `.cas/worktrees/` for any worker whose task has closed cleanly.
- Total disk usage attributable to factory worktrees on the host drops by the sum of leaked worktree sizes after the one-shot sweep. The reporting user's 162-count (and equivalents on other users' hosts) stabilizes at near-zero in steady state.
- No uncommitted work is silently lost — every reclaim of a dirty worktree produces a recoverable `.cas/salvage/*.patch` file.
- Attempts by a supervisor to call `Agent(isolation: "worktree")` are blocked with a clear error; no new `.claude/worktrees/agent-*` directories appear in tracked repos.
- Daemon logs the sweep outcome per cycle (reclaimed count, salvaged count, skipped count with reasons) so drift is observable.
- Feature works on Linux and macOS without code changes; Windows support is explicit best-effort or documented-unsupported (see Scope Boundaries).

## Scope Boundaries

- **Not in scope:** redesigning the factory worker lifecycle or the EPIC merge flow itself. This work sits around those existing pipelines.
- **Not in scope:** deleting branches that are not merged into their parent. Branch cleanup stays tied to merge state, independent of worktree dir removal.
- **Not in scope:** sweeping non-Cassy worktrees (user-created `git worktree add ...` outside `.cas/worktrees/` and `.claude/worktrees/agent-*`). If a user made it by hand, Cassy leaves it alone.
- **Not in scope:** changing Claude Code's own harness behavior around `Agent`/`EnterWorktree`. We work within its existing hook surface.
- **Not in scope:** UI/TUI changes to surface leaked worktree state beyond existing daemon log output (can be a follow-up).
- **Not in scope:** assuming any particular filesystem layout, monorepo root, or directory naming convention. The feature must not hardcode `~/Petrastella`, `~/code`, or any other host-specific path.
- **Not in scope:** assuming the `cas` binary or its config lives in a specific location beyond what Cassy already assumes elsewhere (`~/.cas/` / XDG paths).
- **Windows:** explicit best-effort. Linux and macOS are supported first-class; Windows behavior should not panic but may have limited sweep coverage (e.g., hardlink/junction quirks). Document whatever ships.

## Key Decisions

- **Auto-remove default flips to "yes"**: previous "keep in case of uncommitted work" stance is replaced by "remove if clean, warn + defer if dirty." Rationale: current default caused 162 leaks; dirty tree is the exception, not the rule, and dirty cases are still preserved by R2/R6.
- **FS-driven sweep, not DB-only**: the 162-count proves DB reconciliation alone cannot close the gap. Filesystem is the source of truth for disk usage.
- **Cross-repo reach**: the leak is not local to one repo, so the fix cannot be either. Topology TBD in planning but the requirement is cross-repo.
- **Dirty orphans get salvaged, not deleted in place**: eliminates the "what if I lose work" objection to aggressive reclaim. Patch file is cheap; lost work is expensive.
- **24h TTL for abandonment**: aligns with user's disk-pressure pain; covered by a config knob so projects with longer-running workers can raise it.
- **Block raw Agent worktree-isolation at harness level, not via skill docs**: skill docs already tell supervisors to use `spawn_workers` and the screenshot shows that didn't hold. Enforcement has to be out-of-band from the model.
- **Layered defense for supervisor path**: hook (primary) + daemon detection (backstop) + skill docs (reinforcement). No single layer is trusted alone.

## Dependencies / Assumptions

- **[Assumption — verify in planning]** Cassy has a global registry or discoverable list of "repos it has touched" (the central store at the Cassy data dir and factory socket naming suggest yes, but the exact query path is unconfirmed). Portability requires this be queried from Cassy itself, not derived from a filesystem walk of user directories.
- **[Assumption — verify in planning]** Claude Code exposes a PreToolUse hook surface that can inspect and block `Agent` tool calls based on args (the settings.json hook system documents PreToolUse; args access for `Agent` specifically needs confirmation).
- **[Assumption]** `git diff` + listing untracked files is sufficient to capture salvageable state; binary-file edge cases need handling but don't change the core design.
- **[Assumption]** Cassy's existing data directory resolution (XDG-aware on Linux, `~/Library/Application Support/cas` or equivalent on macOS, already implemented upstream) is the canonical place to write `.cas/salvage/*.patch` on a per-repo basis — i.e., salvage patches live inside each repo's `.cas/salvage/`, not under a central host-level dir.
- `memory/project_factory_worktree_leak.md` (cas-aa65) tracks this as a known issue; this brainstorm supersedes that note for scope.

## Outstanding Questions

### Resolve Before Planning

*(none — all product decisions are closed.)*

### Deferred to Planning

- [Affects R4][Technical] What is the right topology for cross-repo sweep: (a) each per-repo daemon gains a cross-repo scan mode, (b) a new dedicated global sweeper daemon, or (c) a host-level cron/launchd job invoking `cas sweep-all`? Tradeoffs: lifecycle coupling, failure isolation, complexity.
- [Affects R3, R4][Needs research] How does Cassy discover "known repos"? Confirm whether `cas.db` already holds a canonical list, whether `proxy_catalog.json` is authoritative, or whether a new registry is needed.
- [Affects R11][Needs research] Does Claude Code's PreToolUse hook receive structured `Agent` tool args (including `isolation`), or only the tool name? If args aren't accessible, fall back to permission-based deny via `settings.json` scoped to supervisor role.
- [Affects R11][Technical] How is "the caller is a supervisor" detected at hook time? Options: env var, session metadata, process role. Confirm what's actually available in the hook environment.
- [Affects R13][Technical] Is it safe for Cassy to reach into `.claude/worktrees/` given that's Claude Code's directory? Risk: layer violation, breakage if Claude Code changes semantics. Mitigation: treat as opportunistic cleanup with a feature flag.
- [Affects R6][Technical] Salvage patch format — plain `git diff > file.patch` vs `git format-patch` vs bundled tarball for untracked binaries. Pick during implementation.
- [Affects R8][Technical] How does the sweep decide "parent/epic branch" for merge check across heterogeneous repos (main vs develop vs staging)? Probably: use the recorded `parent_branch` on the worktree record when present, fall back to repo default branch.
- [Affects R11, R12][Technical] Exact error text and recovery guidance in the hook block message. Small but user-visible.

## Next Steps

→ Hand off to planning (cas-supervisor or /plan). All product decisions are closed; remaining items are implementation/architecture questions best answered with the codebase open.
