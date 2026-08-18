# Resolution (cas-c145)

**Status:** Fixed on `factory/hv-grok-merge`
**Commit:** `e1b33f4992eb430582ab329e710a1314cd965573`
**Date:** 2026-07-11
**Epic:** cas-0e22

## Characterization (pre-fix stall)

Verified against parent of the fix (`e1b33f4992eb430582ab329e710a1314cd965573^`):

1. **Director WorkerIdle prompt** for MERGE REQUIRED / AwaitingMerge only said “resolve the rejection before acting on the task as closed” — no factory branch, epic target, or merge next-action (`cas-cli/src/ui/factory/director/prompts.rs`).
2. **Event reachability was not the bug** (cas-627f already restored park → idle notification). The **content** of the supervisor nudge was insufficient for Grok to treat merge as top priority.
3. **`GROK_SUPERVISOR_INSTRUCTIONS`** (`crates/cas-pty/src/pty.rs`) mentioned review/merge generically; Grok has no SessionStart additionalContext, so `--rules` is the boot surface and lacked merge-queue-before-user-chat guidance.
4. **Supervisor skill** emphasized never poll / end turn without a complementary push-based merge-drain rule, so free-form user chat could outrank a non-empty merge queue.

## Fix (scope-bounded)

| Layer | Change |
|-------|--------|
| Director auto-prompt | MERGE REQUIRED / AwaitingMerge idle → actionable merge-queue event: task id/title, `factory/<worker>`, epic target branch, `epic_status` + `list status=awaiting_merge`, merge steps, worker re-close, explicit **do not poll** |
| Grok `--rules` | `GROK_SUPERVISOR_INSTRUCTIONS` prioritizes merge-queue drain before free-form chat |
| Supervisor skill twins (Claude/Grok/Codex) | Push-based merge drain in hard rules; Phase 3 workflow documents AwaitingMerge steps |

**Non-goals honored:** close-gate / `close_ops` merge semantics **not** modified (owned by hv-close / sibling tasks). No polling loop introduced.

### Review fix (P1, mixed harness)

`merge_required_idle_prompt_text` now takes both `supervisor_prefix` and `worker_prefix`. Supervisor tools (`epic_status`, list, show) keep the supervisor alias; the worker re-close command uses the **worker** harness prefix so Claude→Codex/Grok factories do not relay an unusable alias. Regression: `test_c145_mixed_harness_awaiting_merge_uses_worker_prefix_for_reclose`.

## Proof (fresh, exit 0)

```
cargo test -p cas --lib ui::factory::director::prompts::tests::test_c145
# → test_c145_awaiting_merge_idle_is_actionable_merge_queue_prompt
# → test_c145_non_merge_close_rejection_stays_informational

cargo test -p cas-pty --lib test_pty_config_grok_supervisor_injects_rules
# → test_pty_config_grok_supervisor_injects_rules_with_cas_prefix (MERGE REQUIRED / awaiting_merge / factory/<worker> / do not poll)

cargo test -p cas --lib test_supervisor_guidance_under_8kb
```

Post-merge, workers re-close via the existing close path (unchanged); verification_flow coverage for post-merge close remains valid.

## Files

- `cas-cli/src/ui/factory/director/prompts.rs`
- `crates/cas-pty/src/pty.rs`
- `cas-cli/src/builtins/{,grok/,codex/}skills/cas-supervisor.md`
- `cas-cli/src/builtins/{,grok/,codex/}skills/cas-supervisor/references/workflow.md`

## Original report (imported from supervisor checkout)

---
from: Ozer Health factory (operator + Grok Build supervisor)
date: 2026-07-10
priority: P0
type: BUG
component: Grok factory supervisor / factory merge queue
project: ozer-health (Richards-LLC/ozer-health)
for_team: cas-src
---

# BUG: Grok supervisor misses `awaiting_merge` / MERGE REQUIRED and stalls the factory

**Label:** `grok-supervisor` · `factory` · `awaiting_merge` · `merge-queue` · **P0**

Please treat this as a **cas-src / Grok Build factory harness** bug, not an Ozer product bug.

---

## Situation (plain language)

We ran a normal Cassy factory epic on Ozer with **Grok Build as supervisor** and **Grok workers**.

Workers did their jobs correctly:

1. Implemented the task
2. Committed + pushed `factory/<worker>`
3. Attempted `cas__task action=close`
4. Got **MERGE REQUIRED** → task moved to **`awaiting_merge`**
5. Messaged the supervisor: “merge us into the epic”
6. Went idle (lease released) — which is **correct**

The **Grok supervisor failed its job**:

- Did **not** merge factory branches into the epic when workers asked
- Answered user chat (“is it on staging?”) while the merge queue was non-empty
- Left the factory looking like “**zero workers are working**” because workers were correctly blocked on the supervisor merge gate
- Only merged after the human escalated multiple times

Human quotes from the same session (evidence of operator impact):

1. “you keep missing messages to merge”
2. “zero workers are working”
3. “supervisor merges to epic, why dont you know how to do your job?”

This is **not** worker death. PIDs were alive, heartbeats fresh. The bottleneck was **supervisor triage**: merge messages were not treated as top priority.

---

## What the supervisor is supposed to do

When a worker hits `MERGE REQUIRED` / `awaiting_merge`:

1. `epic_status` / inspect factory commits
2. Merge (FF / merge / cherry-pick) `factory/*` → `epic/*`
3. Push epic
4. Tell worker to re-close **or** supervisor-close after merge if worker is idle
5. Assign next ready task

That is Phase 3 of factory workflow. Grok supervisor repeatedly skipped it.

---

## Incident details

| Field | Value |
|--------|--------|
| Date | 2026-07-10 |
| Project | Ozer Health (`/home/pippenz/Petrastella/ozer`) |
| Epic | **cas-4c77** — General dosha recipes — dual-mode generation + standalone recipes page |
| Epic branch | `epic/general-dosha-recipes-dual-mode-generation-standal-cas-4c77` |
| Factory session (Cassy) | `ozer-happy-cobra-8` (focus epic cas-4c77) |
| Supervisor agent | `zealous-koala-34` (Grok Build, primary) |
| Supervisor session id | `0869ce7c-11c1-4c74-baf7-e068e7f69781` |
| Workers | `recipe-be` (session `b023b0a7-eeee-4315-a106-2d9c17f1534d`), `recipe-fe` (session `aa6a1071-d8c5-4f99-b874-7b182e96cb36`) |
| Worker model | `cli=grok model=grok-4.5 effort=medium isolate=true` |
| Cassy version | `2.27.0 (9f86e08-dirty 2026-07-10)` |

### Tasks stuck on merge gate (until human forced action)

| Task | Worker | Commit | Status while stalled |
|------|--------|--------|----------------------|
| cas-8eff (backend general recipes API) | recipe-be | `5ec4e032` | `awaiting_merge`, unmerged=1 |
| cas-caec (useRecipeGeneration dual-mode) | recipe-fe | `ea496beb` | `awaiting_merge`, unmerged=1 |
| cas-a5ff (`/recipes` page) | recipe-fe | `c154b6eb` | `awaiting_merge`, unmerged=1 after prior work |

Eventually merges did land (supervisor acted late under pressure). Epic tip after recovery: **`c154b6eb`**.

---

## Logs to read (cas-src team)

All paths are on the operator machine (Petrastella host). Open these first.

### 1. Cassy factory session log (today)

```
/home/pippenz/Petrastella/ozer/.cas/logs/factory-session-2026-07-10.log
```

(~1858 lines, ~106 KB). SessionEnd snapshots + factory activity for 2026-07-10.

### 2. Grok supervisor session (the agent that failed to merge)

Directory:

```
/home/pippenz/.grok/sessions/%2Fhome%2Fpippenz%2FPetrastella%2Fozer/0869ce7c-11c1-4c74-baf7-e068e7f69781/
```

Key files:

| File | Why |
|------|-----|
| `chat_history.jsonl` | Full supervisor conversation (user escalations + tool calls) |
| `events.jsonl` | Tool/event stream |
| `summary.json` | Session summary |
| `prompt_context.json` | What the supervisor was booted with |

Related Grok workspace session (same ozer cwd, later activity):

```
/home/pippenz/.grok/sessions/%2Fhome%2Fpippenz%2FPetrastella%2Fozer/019f4dde-075d-7cd2-9bcb-83f6964482ef/
```

- `chat_history.jsonl`, `events.jsonl`, `updates.jsonl`, `mcp/` tool traces

Workspace-level prompt history:

```
/home/pippenz/.grok/sessions/%2Fhome%2Fpippenz%2FPetrastella%2Fozer/prompt_history.jsonl
```

### 3. Worker worktrees / git

```
/home/pippenz/Petrastella/ozer/.cas/worktrees/recipe-be   # factory/recipe-be @ 5ec4e032
/home/pippenz/Petrastella/ozer/.cas/worktrees/recipe-fe   # factory/recipe-fe @ c154b6eb
```

Remote branches (evidence work was pushed while supervisor still idle):

- `origin/factory/recipe-be`
- `origin/factory/recipe-fe`
- `origin/epic/general-dosha-recipes-dual-mode-generation-standal-cas-4c77`

### 4. Cassy task records (notes show MERGE REQUIRED + re-ping)

Task IDs (query via Cassy in ozer project):

- `cas-4c77` (epic)
- `cas-8eff`, `cas-caec`, `cas-a5ff` (children)
- Tracker for this bug in ozer Cassy: **cas-760e**

Task notes on the children document:

- Close rejected: MERGE REQUIRED
- Worker progress: “Supervisor messaged for merge”
- Later: supervisor escape-hatch close after late merge

### 5. Grep recipes for the transcript

```bash
# User escalations in supervisor chat
rg -n 'missing messages to merge|zero workers|know how to do your job|MERGE REQUIRED|awaiting_merge' \
  "/home/pippenz/.grok/sessions/%2Fhome%2Fpippenz%2FPetrastella%2Fozer/0869ce7c-11c1-4c74-baf7-e068e7f69781/chat_history.jsonl" \
  "/home/pippenz/.grok/sessions/%2Fhome%2Fpippenz%2FPetrastella%2Fozer/019f4dde-075d-7cd2-9bcb-83f6964482ef/chat_history.jsonl"

# Factory log
rg -n 'recipe-fe|recipe-be|cas-4c77|awaiting_merge' \
  /home/pippenz/Petrastella/ozer/.cas/logs/factory-session-2026-07-10.log
```

---

## Expected vs actual

| Expected | Actual |
|----------|--------|
| Worker close → MERGE REQUIRED → supervisor merges same turn or next injected turn | Supervisor ignores merge messages for multiple human turns |
| `epic_status` unmerged>0 is fire-drill priority | Supervisor answers “is it on staging?” while unmerged=1 |
| Workers idle only after next assignment | Workers idle because merge gate blocked close; looks like “nobody is working” |
| No human babysitting of merge queue | Human had to yell three times |

---

## Suspected root causes (for cas-src)

1. **Grok supervisor boot / skill** does not force “scan `awaiting_merge` + `epic_status` before any user reply.”
2. Worker merge messages may not inject as **URGENT** supervisor turns (or Grok ends turn without draining the merge queue).
3. “End turn after assign / don’t poll” guidance without a complementary **start-of-turn merge-queue mandatory scan**.
4. No durable reminder while focused epic has `unmerged > 0`.
5. Possible Grok vs Claude factory harness gap: Claude supervisors may get better merge nudges.

---

## Suggested fix (product / harness)

1. Every Grok supervisor turn **must** begin with:

   ```
   cas__coordination action=epic_status id=<focused-epic>
   cas__task action=list status=awaiting_merge
   ```

   If unmerged > 0 or any awaiting_merge → **merge before answering the user.**

2. Emit first-class factory event `task_awaiting_merge` that injects an URGENT prompt with factory SHA + epic branch pre-filled.

3. Auto-remind supervisor while focused epic has stranded factory commits.

4. Regression test / fixture: simulate MERGE REQUIRED message → assert merge step before free-form chat.

---

## Out of scope

- Ozer recipe product code (that epic’s implementation was fine)
- Squash-merge anchor false negative (separate: `BUG-awaitingmerge-anchor-squash-merge-2026-07-09.md`) — this incident used normal FF merges; the gate was **correct**, the supervisor just didn’t run merges

---

## Contact / context

- Filed by: Grok supervisor session on Ozer at operator request, 2026-07-10
- Operator: human driving factory for epic cas-4c77
- Related ozer Cassy tracker task: **cas-760e**

**Please pick this up in cas-src.** Fix belongs in Grok factory supervisor prompt/skill/harness so merge-queue stalls cannot recur without a human screaming.
