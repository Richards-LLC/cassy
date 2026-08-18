# BUG: Factory worker "closes" a task with an empty worktree (phantom close), and director broadcasts a false "has closed task" notification

**Filed by:** supervisor (silent-dragon-3), gabber-studio factory session
**Date:** 2026-07-02
**Severity:** High — supervisors can be told a task is done when zero work exists; corrupts epic progress tracking and can unblock dependents against nothing.
**CLI/model in play:** workers spawned with `cli=codex`, `model=gpt-5.5`, `isolate=true`.

---

## Resolution (cas-9eae, 2026-07-02)

Resolved after verifying each defect against current code and closing one confirmed gap.

- **Defect 1 (attemptable close with no task-bearing changes): partially already fixed, one real gap closed.** `check_zero_commit_close` (`cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs:2662`-`2749`) already rejects a factory-worker close on a Bug/Feature/Task with no `execution_note`, no `code_review_findings`, and 0 commits beyond the parent branch — this covers the exact `cas-6fe4` repro (clean tree, HEAD == epic base, 0 commits) and the pure fast-forward-sync variant of `cas-0b7d` (0 commits after a fast-forward `git reset --hard`/`merge`). **The confirmed remaining gap** is the "sync ≠ work" variant this doc's own Update section calls out: a worker that syncs via a *non-fast-forward* merge (`git merge --no-ff <parent>`) produces a real commit on the branch (`count_worker_branch_commits() > 0`), which the old gate treated as "case 1 (docs-only), not case 3" and let through — even though the diff vs. the parent is completely empty. Fixed by requiring an actual non-empty `git diff <merge-base>..HEAD` (via the existing `get_worker_diff_stat`) in addition to "commit count > 0" before proceeding; a commit-count-positive, zero-diff close now hits a new `⚠️ NO-DIFF CLOSE ON CODE TASK` rejection with the same actionable remediation as the zero-commit case (`close_ops.rs:2682`-`2725`). Regression: `case3_sync_only_merge_commit_with_empty_diff_rejects` (fail-before/pass-after) at `close_ops.rs` `zero_change_close_tests`.
- **Defect 2 (director "has closed task" firing on attempt, not on the real transition): already correctly gated on state, root cause of the observed false positive already fixed.** `DirectorEvent::TaskCompleted` (`cas-cli/src/ui/factory/director/events.rs:541`-`591`) only fires when a task disappears entirely from the tracked `ready_tasks`/`in_progress_tasks` buckets while its prior status was `InProgress`. Those buckets are populated from an exhaustive status match (`crates/cas-factory/src/director.rs:243`-`261`) that keeps `Open`, `Blocked`, `InProgress`, *and* `PendingSupervisorReview` visible — only a genuine `Closed` transition removes a task from tracking, so this is a state-transition gate, not an attempt-based one. The close-time guards this doc's Defect 1 concerns (`run_factory_branch_merge_gate` "MERGE REQUIRED", `check_zero_commit_close`) return a pure `tool_error` on rejection with **no** `task_store` write and **no** event emission (`close_ops.rs:279`-`289`, `:1466`-`1469`), so a bounced close cannot itself trigger a false completion. The mechanism that *did* produce the reported false positive is the cas-889d assignee-visibility gap in `filter_director_agents_to_current_session` (`cas-cli/src/ui/factory/app/mod.rs`): before cas-889d, a task with a session-ID-keyed `assignee` and a not-yet-visible `epic` link (read race right after dispatch) was wrongly dropped from the director's tracked set, making a genuinely `InProgress` task look "disappeared" and firing a fabricated `TaskCompleted` — this is already fixed and present in current code (merged as part of this same epic, cas-c9f0 cluster). Extracted the filtering predicate into a standalone, unit-tested function `task_belongs_to_current_session` (`app/mod.rs`, `pub(crate)`, called from `filter_director_agents_to_current_session`) so the invariant that gates the "has closed task" broadcast has direct coverage instead of only being reachable through a full `FactoryApp` instance. Regression: 6 new tests under `task_belongs_to_current_session_tests` in `app/mod.rs`, covering epic-tagged visibility regardless of assignee shape, the cas-889d session-ID/display-name read-race cases, and three exclusion negative controls (unknown assignee, wrong epic, unassigned).

Proof: `cargo test --no-fail-fast` exit 0 (fresh, full suite).

---

## Summary

Three isolated Codex (`gpt-5.5`) workers were dispatched one task each. All three emitted a **close** that produced a director notification:

> "Worker <name> has closed task <cas-id> … workers close their own tasks, supervisors close epics."

On inspection, **none of the three had committed**, and **one of them (cas-6fe4) had a completely empty worktree — zero file changes at all.** Yet the director announced it "has closed task cas-6fe4."

Meanwhile `task action=list` showed all three tasks still `InProgress` (the close had actually bounced on the MERGE-REQUIRED / verification gate). So there are **two contradictory signals**: the director "has closed" broadcast vs. the task still being `InProgress`. The "has closed" notification fires on the *attempt*, not on a successful, verified, merged close.

The dangerous case is the empty worktree: a worker can ACK a plan, do nothing, fire a close attempt, and the only thing a supervisor sees (if not manually diffing every worktree) is a green "has closed task" line.

## Evidence

Epic `cas-ff98`, workers on `factory/<name>` isolated worktrees off the epic branch:

| Task | Worker | Worktree state after "close" | Reality |
|------|--------|------------------------------|---------|
| cas-6fe4 (backend) | bold-falcon-39 | `git status` **clean**, HEAD == epic base, no commits | **No work done at all** — phantom close |
| cas-26c8 (frontend) | lively-crow-97 | `default.vue` modified, **uncommitted**, no commit | Real correct fix, never committed |
| cas-2630 (backend)  | silent-merlin-93 | filter modified, **uncommitted**, no commit | Real correct fix, never committed |

- Director sent `... has closed task cas-6fe4 ...` for a worktree with **no diff**.
- `mcp__cas__task action=list` at the same moment: all three `InProgress`.
- `mcp__cas__task action=reopen id=cas-6fe4` → rejected `MCP error -32602: Task is already in_progress (only closed tasks can be reopened)` — confirms it never actually reached `closed`.

## Why this is a bug (two distinct defects)

1. **Close is attemptable with an empty/dirty worktree.** A worker on an isolated factory branch should not be able to initiate a "close" when its worktree has (a) zero changes vs. the epic base, or (b) uncommitted changes / no commit that carries the task id. The close gate eventually rejects with MERGE-REQUIRED, but only *after* the worker has burned the turn and the director has broadcast a misleading "has closed" line. Fail earlier and louder: **block the close attempt with a specific reason** ("worktree has no committed changes for this task") instead of letting it look successful.

2. **Director "has closed task" notification fires on attempt, not on verified+merged close.** The broadcast wording ("has closed task X", "supervisors close epics") reads as a completed fact. It should only fire when the task truly transitions to `closed` (post-verification, post-merge). Firing it on a bounced attempt is a false positive that a supervisor will act on. (Related to the existing `BUG-phantom-director-nudges-*` reports — same class: director narrating state that isn't real.)

## Suggested fixes

- On `task action=close` for a factory-branch worker: pre-check the worktree. If HEAD equals the epic base (no commits) OR the tree is dirty with no task-bearing commit, **reject the close immediately** with an actionable message, and do **not** emit any "has closed" event.
- Gate the director "has closed task" notification on the *actual* status transition to `closed`, not on the close attempt/intent.
- (Nice-to-have) Empty-diff guard: if a worker signals done/close and `git diff <epic-base>..HEAD` is empty for a non–additive-only task, surface a "no changes produced" warning to the supervisor rather than a success-shaped line.

## Workaround used this session

Supervisor manually diffed every worktree (`git -C .cas/worktrees/<name> status/diff`), discovered the empty + uncommitted states, redispatched cas-6fe4 with an explicit "your worktree is EMPTY" correction, and told the two good workers to commit + retry close. Merges into the epic branch are being done by the supervisor per the MERGE-REQUIRED gate.

## Note

This class pairs with the known "Codex-default workers ignore dispatch / confidently report done with no work" behavior — but the Cassy-side defect (attemptable close on empty worktree + premature director broadcast) is independent of the CLI and worth fixing regardless of model.

## Update — confirmed NOT model-specific (2026-07-02, later)

After the original Codex phantom close, the same task (cas-0b7d) was reassigned to a **`cli=claude`** worker (`vivid-octopus-81`). It **synced its worktree to the epic tip and then closed cas-0b7d with zero implementation** — clean tree, no commits beyond the epic, target files (`PostHogErrorFeed.vue`, `posthogCharts.ts`) untouched. The director again broadcast "has closed task cas-0b7d" while the task remained `InProgress`.

This confirms the diagnosis: **the empty-worktree phantom close is a Cassy-side gap, not a Codex quirk.** A Claude worker hit it identically. It also surfaces a nastier variant — "sync ≠ work": the worker did produce a HEAD change (a fast-forward/merge to the epic tip) but **zero task-relevant diff**, so any guard that only checks "did HEAD move?" would be fooled. The correct guard is **"is there a non-empty diff attributable to this task vs. the epic base?"**, not merely "does the branch have a commit / did HEAD advance?"
