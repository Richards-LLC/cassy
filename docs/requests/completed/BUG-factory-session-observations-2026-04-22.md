---
from: abundant-mines factory session (supervisor quick-sparrow-64, session 48456bb1-d1c7-45be-b1af-f95ca3b03301)
date: 2026-04-22
priority: P1
cas_task: (none)
---

# Factory session friction — observations from a real EPIC run

Report from a supervisor that ran two back-to-back multi-worker EPICs (cas-9b16 invite flow batch 3 with 3 workers, then a cas-code-review follow-up EPIC with 3 more workers) on the abundant-mines project today. Every individual issue below is minor in isolation; in aggregate they add real overhead to a factory session. Ordered by impact.

The goal of this report is to give the Cassy team concrete evidence — log excerpts, timestamps, exact supervisor-side workarounds — for issues that are otherwise easy to dismiss as "the user did it wrong."

## 1. Spawn-time `action=mine` race (P1, reproducible every session)

### Symptoms

After `mcp__cas__coordination action=spawn_workers count=N isolate=true`:

1. Supervisor runs `mcp__cas__task action=update id=<task> assignee=<worker-name>` for each worker.
2. Supervisor queues a context-briefing message via `mcp__cas__coordination action=message` for each worker.
3. Within ~1–30 seconds of spawn, each worker runs `mcp__cas__task action=mine` and reports **"No open tasks found"** — then flips to idle, then sits.

Happened in **both** EPIC runs today. Six workers in total, six races.

### Concrete evidence

**Run 1 (17:22 UTC, cas-9b16 EPIC):**

```
17:22:41 — noble-octopus-76  → "action=mine returns no open assignments"
17:22:45 — fair-panther-98   → "no open tasks"
17:22:58 — happy-cheetah-24  → "No assigned tasks, ready for work"
```

Supervisor's `action=update assignee=...` calls at 17:21:xx succeeded (`Updated task cas-a224: assignee, status` etc.). `mcp__cas__task action=show` immediately after showed the correct assignee. The workers still reported empty `action=mine` for 30+ seconds.

**Run 2 (18:14 UTC, cas-4b2a / cas-f1ac / cas-ff31 follow-up EPIC):**

```
18:14:46 — smooth-swan-26    → "action=mine returns no open tasks"
18:14:50 — gentle-puma-76    → "No assigned tasks"
18:14:51 — vivid-marten-83   → "No open tasks. Ready for assignment"
```

Same pattern. All three `action=update` calls had returned `Updated task cas-<id>: assignee` before the workers polled.

### Workaround supervisor had to apply

After the idle notifications arrived, supervisor sent a direct `coordination action=message` to each worker with their exact task ID and an instruction to run `action=start id=<id>`. Workers responded correctly once the message landed, confirming the assignment **was** in the DB — they just hadn't seen it via `action=mine` at spawn time.

### Likely root cause (hypothesis)

- Worker's first `action=mine` runs on a worker-side read path that caches (or reads before) the supervisor's write propagates.
- Or: `action=mine` has a `started_at` / `lease-active` filter that excludes freshly-assigned tasks until the worker calls `action=start` explicitly.
- Or: the worker's first-poll heuristic doesn't wait for coordination queue drain.

Running `mcp__cas__task action=show id=<id>` from the supervisor side always showed the assignment correctly during the window when the worker reported empty `action=mine`, so the write DID land — the read path that `action=mine` uses is the divergent piece.

### Proposed fix

Either:
- **(a)** Worker-side: on first boot, wait for coordination queue drain before running `action=mine`. The coordination-message kick was sufficient in both runs — imply that if you build the initial poll into the boot sequence.
- **(b)** Server-side: make `action=mine` return tasks assigned to the caller regardless of lease/`started_at` state; let the filter shift to a flag (`--only-started`, `--include-pending`).
- **(c)** Document-side: tell supervisors the canonical first-message pattern is "assign via `action=update`, then kick via `coordination message` with the task ID." Today's skill doesn't make the kick explicit, so supervisors have to discover it empirically.

**Supervisor-level cost:** ~45–60 seconds per EPIC burned on wait + kick. In aggregate across today's sessions, ~5 min of dead time.

---

## 2. Factory worker verification jail deadlock under outdated binary (P1 operational)

### Symptoms

In the cas-9b16 EPIC, **every** worker close hit `VERIFICATION_JAIL_BLOCKED`:

```
cas-2599 (fair-panther-98)   → VERIFICATION_JAIL_BLOCKED on close
cas-8e9b (noble-octopus-76)  → VERIFICATION_JAIL_BLOCKED on close
```

The skill docs (`.claude/skills/cas-supervisor/SKILL.md`) explicitly say this is fixed by commit `bba6fbf` (factory worker exemption). The running `cas serve` binary at session start predates that commit.

### Supervisor-side impact

Per the documented "binary outdated" recovery path, supervisor used `mcp__cas__task action=close id=<id> bypass_code_review=true reason="...verification jail deadlock..."`. This works, but:

- Required spawning `task-verifier` subagents manually twice to produce verification receipts before close would accept the bypass. Receipts: `ver-095e38c4d78f` (cas-2599), `ver-7fe0c55780f7` (cas-8e9b). Each cost ~$0.10–0.15 and 2–3 min wall-clock.
- Workers sat idle polling the DB the entire time, generating ~6 idle-notification messages each. Their "VERIFICATION_JAIL_BLOCKED" state is observable and they responsibly did nothing — but they didn't know *why* they were stuck or that supervisor was working on it.

### Observations

- Factory workers polling the DB for task state (as documented by the skill) is the right pattern — they correctly observed the close landed after supervisor bypass and stood down.
- **But**: during the 3–5 minute window while supervisor was verifying + bypassing, workers are both (a) blocking compute and (b) generating message noise that the supervisor has to scroll past.
- Consider: a "close pending — verifier path in progress" status that the worker can observe on `action=show` so they know to quiet down, not keep polling.

### Ask

This isn't a Cassy bug per se — the fix exists, the binary is just out of date. But the operational guidance in the skill should emphasize:

> **Before starting a factory session:** verify `cas serve` is running a binary rebuilt after `bba6fbf`. Run `cargo build --release && restart cas serve` if unsure. An outdated binary will cost ~5 min per closed task to the jail/bypass path.

Currently this note is in the "Worker Failure Recovery" section, easy to miss on a fresh session.

---

## 3. Orphan untracked files from prior factory sessions block cherry-pick (P1 hygiene)

### Symptoms

Twice today, when cherry-picking a worker's branch back to `develop`:

```
error: The following untracked working tree files would be overwritten by merge:
  apps/backend/src/team/team.service.spec.ts
Please move or remove them before you merge.
Aborting
```

Then later:

```
error: The following untracked working tree files would be overwritten by merge:
  apps/backend/src/users/users.service.spec.ts
```

Both files were **leftover uncommitted output** from prior factory session(s) on `develop`, not committed by anyone. Diffing them showed partially-written test files from workers whose sessions ended without a `git clean` or commit.

### Supervisor-side workaround

Read the untracked file + the worker's version, confirmed the worker's version was a superset (or divergent-but-task-current), `rm`'d the untracked file, re-ran cherry-pick. Clean.

### Root cause

Factory worker sessions that end mid-task (user interruption, crash, compaction trigger) leave their work-in-progress on the shared `develop` working tree when operating in non-isolated mode, OR the worktree-based workers were cherry-picking into develop and something broke mid-operation.

### Ask

- When a factory session ends (session_end / shutdown / crash), Cassy could at least *log* what's untracked in the supervisor's worktree so the next supervisor knows there's salvageable state.
- Or: add a `mcp__cas__coordination action=gc_report` variant that surfaces "untracked files in main worktree that appear to be from prior factory work" so supervisor can decide whether to salvage, stash, or delete.

---

## 4. Prior-session WIP on develop working tree (P1 — related to #3)

### Symptoms

Starting this session, `git status` in abundant-mines showed ~30 modified files in the working tree, none committed. Subset was legitimately mine (the swc-jest swap the user requested), but ~5 files were clearly prior factory WIP on lane C / E subtasks that the previous session had not committed:

- `apps/backend/src/invites/invites.service.ts` (+45 lines, lane E email-match hardening)
- `apps/backend/src/users/users.service.ts` (+155 lines, lane E JIT rework)
- `apps/backend/src/workspaces/workspace-access-control.service.ts` (+4 lines, lane C cache work)
- `apps/backend/src/workspaces/workspace.service.ts` (+5 lines, lane C/E shared)
- `apps/backend/src/integrations/qbo-payment-sync.service.ts` (+67 lines, unrelated earlier work)

### Supervisor-side workaround

- Made a selective commit of just my changes (the swc swap) so fresh workers would inherit a clean base.
- Committed the prior WIP as a separate `wip: carry-over from prior factory session` commit so fresh workers could either build on it or discard per-lane.
- **This workaround assumes I can reason about what was prior WIP vs my session's work. A supervisor joining a session cold has no such visibility.**

### Ask

Same surface as #3. A boot-time report of "uncommitted state in the main worktree at session start" would let supervisors triage before spawning workers. Today the only signal is reading `git status` + a lot of cross-referencing against recent task close history to guess which WIP belongs to which lane.

---

## 5. Stale task state (`InProgress` + dead assignee) after session death (P2)

### Symptoms

At session start, three tasks (cas-a224, cas-8e9b, cas-2599) were showing `Status: InProgress` with assignees pointing to workers that no longer existed. `worker_status` confirmed "Workers: None active" + "Filtered stale agent record(s): 3".

Tried `mcp__cas__task action=release id=cas-a224` (and peers): all returned `No active lease found for task cas-a224`. The workers had died without releasing leases, but the **status** was still InProgress. Lease and status had diverged.

Had to use `action=update status=open assignee=<new-worker>` to force both fields at once. Even after that, `action=show` still displayed `Status: InProgress` on the task record (as of the cas-a224 show I ran immediately after the update).

### Ask

- If `action=release` fails because there's no active lease, but the task is `InProgress`, consider auto-cleaning the status to `open` (with an audit note).
- Or: surface a `mcp__cas__task action=reset id=<id>` verb that the docs explicitly recommend for "revive a task from a dead session".
- The `action=show` display-after-write divergence is probably just a read snapshot lag, but it's confusing when you're mid-recovery and trying to confirm the fix landed.

---

## 6. Duplicate / replay coordination messages from "director" (P2 noise)

### Symptoms

Workers reported (unprompted):

> **happy-cheetah-24:** "I'm receiving repeated 'You have been assigned cas-a224' notifications from the director, but the task is already Closed (verified). Ignoring as noise."
> **noble-octopus-76:** "Outbox is replaying stale assignment broadcasts but I'm not re-acting on them."

Workers were smart enough to ignore, but each dup message cost them ~100–500 tokens of context to read and decide "this is old".

### Observation

The team config for this session had two non-supervisor members: `supervisor` (me) and `director`. I (the supervisor) never sent dup messages — worker tooling confirmed the dups came from `director`. It looked like `director` was either replaying the outbox or independently polling+notifying on task state transitions even after close.

### Ask

If `director` is a real agent role, its message-throttle / replay-detection needs tightening. If it's a ghost from a prior session, agent GC should clean it.

---

## 7. `bypass_code_review=true` required for supervisor-close on worker-verified work (P2 UX gap)

### Symptoms

When supervisor-closing a task whose **worker already ran `cas-code-review` autofix in their own session** (with ReviewOutcome envelope produced, no P0s, 3 safe_auto fixes applied), the close still required `bypass_code_review=true` with an explicit justification. Twice today: cas-2599 and cas-9b16 (epic).

### Why this is friction

- The worker *already did* the code review. Their envelope is attached to their close attempt's notes. There's no reviewer gap — there's a missing API shape.
- Re-running `cas-code-review` on the same diff from the supervisor side wastes ~$0.30–$0.50 in Sonnet calls for no additional signal.
- Forcing `bypass_code_review=true` with a justification string is the right escape hatch for "this review literally doesn't apply," but it's the wrong shape for "the review already happened, just in a different session."

### Ask

- A supervisor-close variant that accepts `code_review_findings: <ReviewOutcome JSON>` from a cited earlier review run (e.g., `code_review_findings_from_task_note: 1`), so the gate can validate that a review *did* happen without re-running it.
- Or: when a worker fails to close because of verification jail but their cas-code-review ran successfully in-session, persist the envelope on the task so the subsequent supervisor-close can forward it.

### Relatedly, for epics specifically

Epic-close (cas-9b16) also hit this gate. Epics don't have unique diffs — they're the union of already-reviewed subtask diffs. The gate probably shouldn't fire for epic close at all, or should fire on "any unclosed subtask with residual code-review findings" instead of "the epic itself has reviewable changes."

---

## Summary for triage

| # | Issue | Severity | Session cost today |
|---|---|---|---|
| 1 | Spawn race on `action=mine` | P1 | ~1 min × 2 sessions = 2 min |
| 2 | Verification jail under outdated binary | P1 operational | ~8 min × 2 closes = 16 min |
| 3 | Orphan untracked files blocking cherry-pick | P1 hygiene | ~3 min × 2 occurrences = 6 min |
| 4 | Prior-session WIP on develop working tree | P1 hygiene | ~5 min triage at session start |
| 5 | Stale `InProgress` + dead assignee | P2 | ~2 min × 3 tasks = 6 min |
| 6 | Director replay / dup messages | P2 | minor context drain per worker |
| 7 | `bypass_code_review` for worker-verified close | P2 UX | ~2 min × 2 closes + wasted tokens |

**Total supervisor time on friction: ~40 min of an otherwise-productive session.** Most of that is recoverable with (1), (3), and (7) alone.

Happy to clarify or produce additional evidence on any of the above — I still have the session transcript at `/home/pippenz/.claude/projects/-home-pippenz-Petrastella-abundant-mines/48456bb1-d1c7-45be-b1af-f95ca3b03301/`.
