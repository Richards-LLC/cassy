---
title: Factory Close-Merge Gap & Verification Jail Recovery — Research
date: 2026-05-01
status: scoping
author: tender-owl-6 (supervisor) + investigation agents
related:
  - docs/requests/BUG-verification-jail-recovery-factory-branch-merge-gap.md (P1, gabber-studio 2026-05-01)
  - docs/verifier-dispatch-trace.md (2026-04-08, prior analysis of bba6fbf)
---

# Factory Close-Merge Gap & Verification Jail Recovery

## Why this document exists

Cross-team P1 BUG report from gabber-studio (`docs/requests/BUG-verification-jail-recovery-factory-branch-merge-gap.md`, 2026-05-01) describes a silent data-loss vector: `task action=close bypass_code_review=true` marks tasks as Closed in the DB but does **not** verify that the worker's `factory/<worker>` branch was merged into the parent epic branch. In epic `cas-6e07`, 5 of 11 closed tasks had stranded factory commits totaling ~3000 LOC, including the architectural anchor (T5) and Playwright capstone (T7).

This research synthesizes:
1. What the bug report claims
2. Code-level investigation of the close/jail/coordination surface
3. Field evidence from real worker/supervisor session logs across **gabber-studio**, **cas-src**, and **ozer** to validate scope, severity, and frequency

The goal is to scope an EPIC that fixes the *actual root cause*, not just the reported symptom.

---

## Part 1 — What the bug report claims

### Symptom
- Supervisor closes via `mcp__cas__task action=close bypass_code_review=true` during verification-jail-recovery
- DB row → Closed, lease released
- Working tree / git refs untouched → factory branches strand commits
- Both worker and supervisor see "Closed" and assume merged

### Lifecycle (per report)
1. Worker commits to `factory/<worker>` ✅
2. Worker runs `mcp__cas__task action=close` → **VERIFICATION_JAIL_BLOCKED**
3. Worker forwards close to supervisor per `recovery.md`
4. Supervisor runs `task action=close bypass_code_review=true` → SUCCESS
5. ⚠ No git operation has occurred. `factory/<worker>` is stranded.

### Concrete incident (cas-6e07, 2026-05-01)
- 7 stranded factory branches (cas-76a0, cas-4e48, cas-7625, cas-1036, cas-99c8, cas-f4f9, cas-afe1)
- ~3000 LOC of approved/reviewed work structurally invisible to the epic
- Caught accidentally during a worker rebase, not via the supervisor checklist
- Recovery: ~45 min coordination + 3 rebase race-cycles (epic kept moving)

### Proposed fix (gabber-studio's recommendation)
- **Option A (preferred):** hard-block close if factory branch has unmerged commits; `--skip-merge-check` escape hatch with audit-log
- **Option B:** soft-warn + auto-create MERGE follow-up reminder
- **Option C:** read-only `coordination action=epic_status epic_id=<id>` audit surface

Recommends **A + C combined**.

### Secondary observations (worth triage)
1. **VERIFICATION_JAIL_BLOCKED itself is a bug** — workers can't always close their own task even after passing all gates locally
2. **`task action=reset`** returned `lease_released: true` but lease still appeared locked from worker's view
3. **`verification action=add`** accepts task IDs from supervisors *sometimes*, rejects with "Supervisors can only verify epics, not individual tasks" other times — appears assignee-state-dependent

---

## Part 2 — Code-level investigation

### 2.1 Close path & insertion seam

Source: `cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs`

| Concern | File:Line | Notes |
|---|---|---|
| MCP entry | `cas-cli/src/mcp/tools/service/mod.rs:285` | dispatch to `cas_task_close()` |
| Handler | `close_ops.rs:147` | `pub async fn cas_task_close(...)` |
| Existing pattern | `close_ops.rs:161-182` | `check_unmerged_epic_branches()` for epic close — **template** for worker-task version |
| Recommended insertion | `close_ops.rs:183` | between epic-unmerged check and verification policy load |
| Status transition | `close_ops.rs:837-910` | DB → Closed; **no git ops in close path** today |
| Bypass auth | `close_ops.rs:1549-1574` | Only supervisors; appends decision-note to `task.notes` (audit trail = notes field, not separate log) |

Confirms the bug report: close path performs zero git operations on worker-task closes.

### 2.2 Factory branch derivation

Source: `cas-cli/src/worktree/manager/worker_ops.rs:43-46`

```rust
pub fn branch_name_for_worker(&self, worker_name: &str) -> String {
    format!("factory/{worker_name}")
}
```

Worker name = `task.assignee` (display name like `mighty-viper-52`). No code currently derives factory branch from assignee for verification/merge checks; we'd construct at close time.

### 2.3 Git access pattern (already established)

Source: `close_ops.rs:1383-1408`

Existing close-path code shells out to `git` via `std::process::Command::new("git")`, sets `.current_dir(worker_worktree_path)`, calls `.output()`, parses stdout. Pattern:
- `git merge-base HEAD <parent-branch>`
- `git diff --name-status <merge_base>..HEAD`

No external git library dependency. Reuse this pattern for the new merge-state check.

### 2.4 Coordination tool surface

Source: `cas-cli/src/mcp/tools/service/mod.rs:439-594`

Three action domains: agent / factory / worktree. Factory domain (lines 489-507) already has read-only audit precedents (`gc_report` lines 765-860, `worker_status`, `worker_activity`). New `epic_status` slots in here.

### 2.5 Subtask traversal

Source: `crates/cas-store/src/task_store.rs:944` — `get_subtasks(epic_id)` already implements the recursive `WITH RECURSIVE … parent-child` query. Use as-is.

### 2.6 Supervisor checklist gate (today: manual)

Source: `.claude/skills/cas-supervisor-checklist/SKILL.md:88-90`

```
## Before Closing an EPIC
- Verify all worker branches are merged into the epic branch
- Confirm task deliverables exist on the epic branch
- Run full test suite on epic branch
```

Currently a mental step. `epic_status` would automate the first sub-bullet.

### 2.7 ⚠ The regression that nobody flagged in the bug report

Source: `cas-cli/src/mcp/server/mod.rs:617-686` (`authorize_agent_action`)

**Commit `bba6fbf` (2026-04-03)** exempted factory workers from `VERIFICATION_JAIL_BLOCKED` for **all** mutating operations — including `task.close`. Per `docs/verifier-dispatch-trace.md:60-104`, the jail removal broke the only forcing function for verifier dispatch:

> `cas_task_close` returns `⚠️ VERIFICATION REQUIRED` text but nothing compels the out-of-band verifier spawn — the text-based dispatch relies on the jail as a forcing function.

`verifier-dispatch-trace.md:211-216` itself flags `bba6fbf` as "mistake or intentional scope creep" and recommends narrowing the exemption to non-close mutations.

**Implication:** rocketship's stranded-commits incident is partially *caused* by this regression. With the jail intact for `task.close`, workers would have been forced into the verifier-dispatch path, passed it, closed their own task, and merged their own branch. Instead they all routed through bypass-close — which never had the merge check.

The originally-reported "bug" (close doesn't check merge state) is real, but the *reason supervisors are constantly using bypass-close* is the regression.

### 2.8 Verification action=add inconsistency — confirmed

Source: `cas-cli/src/mcp/tools/core/workflow/verification_tools.rs:5-74`

```rust
// line 54
assignee_inactive = task.assignee.as_deref().map(...).unwrap_or(true);
```

When `task.assignee` is `None`, `unwrap_or(true)` treats it as inactive → bypasses the supervisor-only-epics rejection. Bug report's hypothesis confirmed: behavior depends on assignee presence at call time. The rule is sensible (don't steal verification from a live worker) but the error message lies.

### 2.9 Reset lease divergence — possible race, needs field validation

Source: `cas-cli/src/mcp/tools/core/agent_coordination/task_claiming.rs:408-465`

`release_lease_for_task` returns `Ok(true)` if a lease existed, `Ok(false)` otherwise; both are success. No documented in-process cache. Likely SQLite connection-isolation race or stale message-queue artifact. Cannot confirm fix without field reproduction.

---

## Part 3 — Initial scoping (pre-log review)

### Proposed EPIC: factory close-merge gap + verification jail recovery

| ID | Task | Priority | Notes |
|---|---|---|---|
| T1 | Hard-block close on stranded factory commits | P1 | Insertion at `close_ops.rs:183`, bypass does NOT skip; new `--skip-merge-check` flag with audit-note |
| T2 | New `coordination action=epic_status epic_id=<id>` | P1 | Slot in `factory_ops.rs` alongside `gc_report`; wire into supervisor checklist |
| T3 | Narrow `bba6fbf` exemption: re-jail `task.close` for factory workers | P1 | **Root cause.** Different file from T1; same conceptual area |
| T4 | Fix `verification action=add` supervisor authz error message | P2 | ~30 LOC; tighten + clarify rule rather than loosen |
| T5 | Investigate `task action=reset` lease divergence | P3 (research) | Needs field repro; backlog |

### Sequencing
- Wave 1 (parallel): T1, T2, T4
- Wave 2: T3 (after T1 design aligns; needs end-to-end close path test)
- T5 stays in backlog

### Open questions for team-lead
1. Ship T3 in this EPIC or spin out separately? (`bba6fbf` revert has risk; the original commit had a reason)
2. Keep `--skip-merge-check` escape hatch on T1? (less load-bearing if T3 ships)
3. File `docs/requests/` reply to gabber-studio now or after EPIC closes?

---

## Part 4 — Field evidence from session logs

_Three parallel investigation agents mined recent worker and supervisor session JSONLs and project-local Cassy SQLite DBs across **gabber-studio**, **cas-src**, and **ozer**. The picture got worse than the bug report described, and clearer about root cause._

### 4.1 gabber-studio — confirms report, **incident count is higher**, and **NOT first occurrence**

Primary log: `~/.claude/projects/-home-pippenz-Petrastella-gabber-studio/432f2327-7b67-47b6-b27a-bd45496f9ea2.jsonl` (supervisor, 11.7MB).
Primary DB: `gabber-studio/.cas/cas.db` (project-local; the global `~/.cas/cas.db` does NOT contain cas-6e07).

**Confirms:**
- **VERIFICATION_JAIL_BLOCKED** is a constant on `task.close` — **118 hits** in the supervisor log. Sample tasks bouncing on close: cas-2f38, cas-edb8, cas-7bec, cas-ad02, cas-17d4, cas-62958.
- **The stranded-commits incident reproduces** exactly. Direct quote at 2026-05-01T17:29:40Z:
  > _"5 of those 'closed' tasks have commits stranded on factory branches that never merged to epic — including the architectural anchor (T5 funnel-session) and the regression capstone (T7 e2e). [...] T4 cas-76a0 (4 commits on factory/happy-jay-14), T5 cas-7625 (5), T7 cas-1036 (3), T9 cas-4e48 (3), T10 cas-99c8 (2), cas-f4f9 (2)"_
- **Recovery race confirmed**: 2026-05-01T17:43:26Z — _"Your worktree's epic ref is stale. PR #862 IS already merged to origin/epic/sms-conversion-funnel"_ — the supervisor catching subtle-marten-74's base being obliterated mid-rebase. Three rebase cycles to land safely.
- **45-min recovery time confirmed** by timestamps: discovery at 17:27:50Z → wrap-up 17:59:41Z (~32 min active) + report-writing → ~45 min total.
- **`verification action=add` rejection confirmed** at 2026-05-01T17:15:42Z with the exact error string. Other tasks accepted via the same flow same session (cas-7625, cas-76a0, cas-cf61, cas-afe1) — reproduces the assignee-state-dependent inconsistency.

**Extends (worse than reported):**
- **9+ bypass-close events in cas-6e07**, not 5. SQL on `verifications` table for the cas-6e07 window:
  ```
  SELECT COUNT(*) FROM verifications WHERE summary LIKE '%bypass%' AND created_at > '2026-05-01T16';
  → 9
  ```
  Bypass-closed task IDs: cas-8d9a, cas-cf61, cas-afe1, cas-76a0, cas-7625, cas-99c8, cas-f4f9, cas-e152, cas-1036, cas-4e48, cas-9343, cas-e0fb, cas-9522, cas-2410, cas-11a4, cas-e224, cas-0655.
- **7 stranded tasks**, not 5: noble-sparrow-57 caught a 7th (cas-afe1, T3 ownership guard, 2 commits) during PR cleanup that the supervisor's initial survey missed.
- **NOT first occurrence.** At 2026-05-01T14:03:54Z — *hours before* cas-6e07 began — the same supervisor wrote:
  > _"When I 'closed' cas-2342 yesterday I went through verification + supervisor close, but the actual `gh pr create` + merge step was never done [...] solid-panda-93's real fix was 130+ lines, sitting on the factory branch unmerged."_

  cas-2342 was stranded on **2026-04-30**, surfaced via user retest (_"dude, same exact issue"_), fixed by cherry-pick to PR #840. **At least the second occurrence in 48 hours.**
- **gabber-studio already filed `cas-9a0a` (P2)** at 2026-05-01T17:29:25Z proposing the same fix being scoped here.

**Absent:** No `lease_released` / lease-still-locked sequences in this session's logs. The bug report flagged that as "secondhand"; field evidence does not corroborate.

### 4.2 cas-src — the team already diagnosed this **3 weeks ago**

**The cas-src team independently identified the bba6fbf regression on 2026-04-08** — three weeks before the gabber-studio incident — and has an open EPIC tracking it. We are not first responders; we're closing a known issue that already cost the cas-src team 8 min × 2 closes on 2026-04-22.

**Existing artifacts (already in tree):**
- `docs/verifier-dispatch-trace.md` (247 lines, 2026-04-08) — names commit `bba6fbf`, lines 211-216 explicitly call it "mistake or intentional scope creep" and recommend narrowing the exemption to non-close mutations
- `docs/requests/BUG-factory-session-observations-2026-04-22.md` (244 lines) — internal twin of the gabber-studio report, written 9 days earlier
- `cas-cli/src/builtins/skills/cas-supervisor.md` (uncommitted edits) + new `references/preflight.md`, `references/worker-recovery.md` (2026-04-29) — user-facing mitigations only, not code fixes
- Open EPIC `cas-9508` ("epic-factory-session-friction-spawn-race-jail-cost") since 2026-04-23
- Three stub `epic/factory-*` branches with **0 commits beyond main** — placeholders, **no duplicate code work in flight**

**Live bypass-close events in cas-src dogfooding:**
- `cas-b65f` EPIC close — 2026-04-29T17:58:33Z, supervisor `cas-src-mighty-cardinal-71`, the v2.10.0/2.10.1 release ship (commits `da10335`, `858c7c7`)
- `cas-d24a` — 2026-04-27T19:41:08Z, supervisor `cas-src-kind-parrot-78`, "not-a-bug" investigation close

**Why no data loss in cas-src's own logs:** `git branch --list 'factory/*'` returns empty. The v2.10.x EPICs merged worker branches *before* close. The bug fired on the same code path but didn't lose work because of release-pipeline discipline, not because the path is safe.

**Skill-level documentation already acknowledges the cost:**
> _"The canonical instance of this was 2026-04-22 (~8 min × 2 closes wasted) when `cas serve` predated commit `bba6fbf`… every worker will hit `VERIFICATION_JAIL_BLOCKED` on close, and you will burn time running `task-verifier` manually and adding `bypass_code_review=true` overrides that shouldn't be necessary."_
> — `cas-src/cas-cli/src/builtins/skills/cas-supervisor/references/preflight.md`

This is the *system maintainer team* paying the same tax and writing skill docs as a workaround.

### 4.3 ozer — heavy symptom, **zero data loss**, reveals workflow-dependent risk

Ozer hits the symptom (VJB, bypass-close) at higher volume than the other two projects but has lost nothing. The reason is workflow discipline, not a safer code path.

**Symptom volume (VJB hits + bypass-close):**
- **113+ VJB events** across 29 main session files + 11 worktree sessions; always on `task.close`, never another mutation
- **~28+ `bypass_code_review=true` close events** across 13 sessions
- Heaviest: E2 wallet run 2026-05-01 (`4f4c906b-…jsonl`, 20 hits), E1 payment foundation 2026-04-29 (`0397e2bd-…jsonl`)

**Smoking-gun check: any stranded commits?** **No.** Verified against the git tree:
- All 5 E2 factory branches: 0 unmerged commits vs `epic/e2-native-wallet-checkout-…`
- All 4 review-persona branches: 0 unmerged commits vs E1, E2, or main
- Each bypass-closed task (cas-c1f4, cas-9b23, cas-3c53, cas-802a, cas-bf5b3) has 2-6 commits reachable on the epic via PR-merge

**Why ozer is safe (and gabber-studio wasn't):** ozer's supervisor uses three undocumented mitigations:
1. **PR-merge-first**: merge `factory/<worker>` into `epic/<slug>` via `gh pr merge` BEFORE running bypass-close. By the time the task closes, the merge is already on origin.
2. **Manual `git diff --stat` verification**: ozer memory file (`feedback_factory_workers_review_tasks_unreliable.md`) explicitly says _"Verify via `git diff --stat` against the parent branch before closing — confirms scope discipline matches close-notes."_ A human running the check the code should run.
3. **Branch convention**: ozer uses `epic/<slug>` for child task branches; `factory/<worker>` exists only as ephemeral worktrees. Less surface area for stranding.
4. **No Cassy worktree-lease subsystem reliance**: `SELECT COUNT(*) FROM worktrees` = 0 in ozer's `cas.db`. Supervisor manages branches via `gh` directly — sidesteps the lease-divergence path entirely.

**Notable absences in ozer:**
- 0 "Supervisors can only verify epics" rejections (that string only appears in cas-src and gabber-studio logs)
- 0 `lease_released`-then-lock-complaint sequences

**Implication:** the bug is **systemic** but **probabilistic**. Any project that runs bypass-close *before* merging — like gabber-studio's "close everything, then catch up on PRs" pattern — loses work silently. Projects with PR-merge-first discipline (ozer) paper over the gap. Discipline is not a feature; one slip = silent loss.

### 4.4 Cross-project synthesis

| Symptom | gabber-studio | cas-src | ozer |
|---|---|---|---|
| VJB on `task.close` (118 / many / 113+) | ✅ | ✅ | ✅ |
| `bypass_code_review=true` usage frequency | high | low (release ships only) | high (~28 events) |
| Stranded factory commits | **7 confirmed (≥2 incidents)** | 0 | 0 |
| `verification action=add` authz inconsistency | confirmed (cas-e152) | confirmed | not observed |
| `task action=reset` lease divergence | reported, not in logs | not observed | not observed |
| Mitigation in place | none — caught by accident | release pipeline merges first | PR-merge-first + manual diff |

**The story:**
1. Commit `bba6fbf` (2026-04-03) silently broke verification-jail forcing on `task.close`.
2. Three projects independently noticed the *symptom* (VJB + bypass-close usage spike) by 2026-04-22.
3. cas-src diagnosed the root cause in `verifier-dispatch-trace.md` on 2026-04-08, opened EPIC `cas-9508` on 2026-04-23, but shipped only skill-level workaround docs.
4. gabber-studio paid the data-loss tax twice (cas-2342 on 2026-04-30, cas-6e07 on 2026-05-01) — each caught by accident, not by the system.
5. ozer hits the symptom heavily but workflow discipline (PR-merge-first) papered over the gap.
6. The bug report we received is **the second incident** in 48 hours, escalated to P1 because someone finally noticed mid-stream.

**Severity is materially higher than the original report suggests.** This is not a 1-project anomaly; it's a known regression with a 3-week-old root-cause analysis, a cross-project pattern, and at least one undocumented near-miss outside gabber-studio's logs (cas-2342).

---

## Part 4.5 — T3 is already shipped (verified 2026-05-01)

After Part 4 was written, we verified the actual git state of `authorize_agent_action`. **The bba6fbf narrowing is already in main and shipped.**

- **`bba6fbf` (2026-04-03)** — introduced the regression: blanket factory-worker exemption from VJB
- **`8ee0c8f` "Fix task.close verifier dispatch regression (cas-4acd)"** — narrowed the exemption with the exact clause we'd write:

  ```rust
  // server/mod.rs:651-668 (current main)
  // Factory workers are exempt for most mutations [...] However, `task.close`
  // itself is NOT exempt: that's the one call where the jail must still fire,
  // because close is what triggers verifier dispatch. Exempting close here was
  // the bba6fbf regression that broke dispatch for factory workers entirely [...]
  if is_factory_worker && !(tool == "task" && action == "close") {
      return Ok(());
  }
  ```

- Fix shipped in **v2.10.0**, currently running in v2.10.1 (`cas 2.10.1 (49c27d8-dirty 2026-05-01)`)
- Related fixes also landed: `8f6de5d` (supervisor bypass writes `Skipped` row so downstream workers don't get jailed by stale `pending_verification`) and `97aa0d5` (PID-reuse-resistant liveness gate)

**Implication for the bug report:** the gabber-studio 2026-05-01 incident is **post-fix**. The narrowing is working — workers ARE hitting VJB on close (correct behavior). They spawn the verifier. **The supervisor reaches for `bypass_code_review=true` anyway** because verifier runs are slow (~8 min × N tasks per the existing skill doc). Bypass closes the task, doesn't merge, factory branches strand.

So the real story is:
- bba6fbf broke verifier dispatch (silent skip)
- 8ee0c8f restored dispatch (jail forces it)
- The forced-but-slow verifier became friction supervisors route around via bypass
- Bypass became the new silent-loss path
- **T1 + T2 are the actual remaining fix.** T3 is a no-op — the work is already done.

---

## Part 5 — Updated scoping after field evidence

### Adjustments to original scope

1. **T3 (revert bba6fbf overreach) — STRIKE. Already shipped in `8ee0c8f` / v2.10.0.** Verified in source. No work needed. The doc text we'd have written matches the comment already in main. Convert T3 into a small audit task: confirm both the narrowing and `8f6de5d`'s Skipped-row write are exercised in tests, and verify gabber-studio's `cas serve` is on v2.10.0+ (their incident has the right symptoms for a post-fix run, but worth a one-line check).

2. **T1 should NOT have a `--skip-merge-check` escape hatch.** With T3 fixing the root cause, the legitimate need for bypass-close drops dramatically. An escape hatch would let the same-day workaround pattern persist. If a real edge case emerges, it's better to surface it as a bug than to ship a silent escape.

3. **T2 (`epic_status`) wires into the supervisor checklist as MANDATORY pre-close gate, not a discoverable command.** Ozer's experience shows that even with discipline, the gap is one slip away. Make the audit automatic: invoke `epic_status` before any epic close.

4. **T4 (verification authz error message) — fix is straightforward:** rewrite the error to say what it actually does (_"Cannot verify task with active assignee X. To take over, release their lease first or wait for them to disconnect."_).

5. **T5 (lease-reset divergence) downgraded to BACKLOG.** Three projects, no field evidence. The bug report flagged it secondhand; the original supervisor didn't reproduce it themselves. Park.

6. **NEW T6: Reconcile with EPIC `cas-9508`.** That EPIC has been open since 2026-04-23 with 3 placeholder branches and no commits. Either retire `cas-9508` and absorb its scope into the new EPIC, or roll the new tasks under `cas-9508`. Don't run two parallel EPICs on the same root cause.

7. **NEW T7: Reply to gabber-studio inbox.** Document our findings (this incident is #2 in 48h, root cause is bba6fbf, EPIC sequencing) so they don't think their report disappeared.

### Sequencing (revised post-T3-strike)

- **Wave 1 (parallel, 2 workers):** T1 (close-merge guard), T2 (epic_status as hard gate + callable, checklist wire-up)
- **Wave 2:** T4 (verification authz error message) — small, can pair with whichever worker finishes Wave 1 first
- **Pre-work (supervisor):** T6 (retire `cas-9508`, supersede with new EPIC; team-lead approved this 2026-05-01)
- **Post-merge:** T7 (reply to gabber-studio inbox + audit task confirming v2.10.0+ on their `cas serve`)
- **Backlog:** T5 (lease-reset divergence — no field evidence)

### Estimate

~1 supervisor day with 2 workers in parallel. T1 has the most risk (touching `close_ops.rs` close path); allocate the senior worker.

### T2 design decision: hard gate AND callable command

**Best for Cassy robustness.** Three projects, three patterns: gabber-studio (no discipline → data loss), cas-src (release-pipeline discipline → no loss), ozer (PR-merge-first discipline → no loss). Only system-level enforcement covers all three.

- **Hard gate at epic close** — mirrors the existing `check_unmerged_epic_branches()` pattern at `close_ops.rs:161-182`. Today Cassy already hard-blocks epic close on unmerged branches at the epic level; T2 extends the same principle from "epic branch" to "every child task's factory branch." Same shape, no precedent shift.
- **Callable command** — same logic exposed for in-flight diagnostics (rocketship's original ask). Cheap: same code, two call sites.

Callable-only is strictly weaker than gate+callable. Discipline failed twice in 48 hours; the system should enforce.

### Open questions for team-lead (resolved 2026-05-01)

1. ~~`cas-9508` reconciliation~~ → **Retire and supersede.** Confirmed.
2. ~~Ship T3 in this EPIC?~~ → **Already shipped in `8ee0c8f` / v2.10.0.** No work needed; converted to T7 audit.
3. ~~`epic_status` auto-gate vs callable?~~ → **Both.** Hard gate at epic close + callable command for in-flight diagnostics. Best for Cassy robustness.

Ready to create the EPIC and spawn workers.
