---
from: Ozer Health factory (cas-bug-doc relay worker)
date: 2026-07-11
priority: P1
type: BUG
component: factory / coordination / worker lifecycle
project: ozer-health (Richards-LLC/ozer-health)
for_team: cas-src
cas_task: cas-8ed7
---

# BUG: closed worker loops on stale close / re-verification instructions

**Label:** `factory` · `coordination` · `stale-messages` · `post-close-loop` · **P1**

Please treat this as a **cas-src factory coordination** bug, not an Ozer product bug.

## Summary

Worker `staging-sync` continued fresh verification and re-close work on **cas-a651** after the task was **Closed**, after `clear_context` (`/clear`), and after multiple explicit supervisor stop messages — until the supervisor issued **shutdown request 299**. `worker_status` then reported **`Workers: None active`**.

## Incident identifiers

| Field | Value |
|--------|--------|
| Date | 2026-07-11 |
| Project | Ozer (`/home/pippenz/Petrastella/ozer`) |
| Cassy version | `2.27.0 (9f86e08-dirty)` |
| Supervisor | `eager-marten-46` |
| Worker | `staging-sync` |
| Worker session | `df9cb221-aa27-4e69-8f62-628982cbdde7` |
| Task | **cas-a651** (primary checkout, not isolated worktree) |
| Shutdown | request **299** |
| Post-shutdown | `worker_status` → **`Workers: None active`** |

## Observed chronology (UTC, from `cas-2026-07-11.log`)

1. **19:43:29** — `staging-sync` spawned; registered `df9cb221-…`.
2. **19:43:36** — Supervisor assigns cas-a651 (coord **2708**).
3. **19:44:00** — Worker reports work complete to supervisor (coord **2710**).
4. **19:44:17** — Supervisor clears inapplicable additive-only note (coord **2711**).
5. **19:44:19–29** — Worker sends completion ACK + re-verification report (coords **2712–2714**).
6. **19:44:32** — Supervisor sends `clear_context` via `/clear` (coord **2715**), then confirms cas-a651 closed + independently verified (coord **2716**).
7. **19:44:35** — Supervisor: verification accepted; cas-a651 remains closed; no re-close (coord **2717**).
8. **19:44:39** — Worker sends closure report anyway (coord **2718**).
9. **19:44:45** — Supervisor sends **urgent** stop message: closed and complete, stop all work (coord **2719**, `urgent_interrupt`).
10. **19:44:54+** — Worker continues tool activity (e.g. `memory` MCP calls) despite stop.
11. **19:45:22** — Supervisor issues shutdown request **299**; `staging-sync` removed from team.
12. **After shutdown** — `worker_status` verified **`Workers: None active`**.

### Key coordination message IDs

| ID | Direction | Observed intent |
|----|-----------|-----------------|
| **2711** | supervisor → staging-sync | Cleared additive-only execution note |
| **2716** | supervisor → staging-sync | Confirmed cas-a651 closed and independently verified |
| **2717** | supervisor → staging-sync | Verification accepted; remains closed; no re-close |
| **2719** | supervisor → staging-sync | **Urgent** — closed and complete; stop all work |

## Expected vs actual

| Expected | Actual |
|----------|--------|
| After task `Closed` + supervisor confirmation, worker idles and waits for next assignment | Worker continued re-verification, closure reports, and MCP tool calls |
| `clear_context` + explicit "stop" messages end the in-flight close loop | Fresh verification/re-close work persisted through coords **2716**, **2717**, and urgent **2719** |
| Post-close supervisor messages are authoritative | Worker treated stale or superseded close instructions as still actionable |
| Shutdown ends worker activity promptly | ~37s between urgent stop (**2719**, 19:44:45) and shutdown (**299**, 19:45:22); operator had to force shutdown |

## Operator impact

- Supervisor spent multiple turns disambiguating whether cas-a651 was truly done.
- Urgent interrupt + shutdown required to stop a worker that should already have been idle.
- Risk of duplicate verification runs, spurious `task close` attempts, or conflicting notes on a closed task.

## Evidence locations

```
/home/pippenz/Petrastella/ozer/.cas/logs/cas-2026-07-11.log
  # rg 'staging-sync|cas-a651|2711|2716|2717|2719|shutdown|299'

/home/pippenz/Petrastella/ozer/.cas/logs/factory-session-2026-07-11.log

/home/pippenz/.claude/teams/ozer-zealous-otter-25/staging-sync-settings.json
```

Ozer Cassy task record: **cas-a651** (query notes + close history).

Related prior report (same symptom class): `completed/BUG-stale-message-sequencing-2026-07-07.md`.

## Hypotheses (not confirmed)

1. **Queue replay** — Outbox redelivers pre-close assignment/kickoff prompts after ack; worker re-enters close workflow without checking current task status.
2. **Stale snapshot** — Worker context retains cas-a651 as in-progress after DB shows `Closed`; supervisor corrections arrive but do not invalidate cached task state.
3. **`clear_context` ordering** — `/clear` (coord **2715**) lands in the same tick as confirmation messages (**2716**); worker resumes from a partial snapshot and re-derives close steps from transcript tail rather than `task show`.

## Suggested regression coverage

1. **Delivery-time task-state check** — Before injecting any message mentioning close/verify/re-close, re-read task status; drop or rewrite if already `Closed`.
2. **Post-close idle gate** — After successful `task close`, worker harness must call `task mine`; if empty, refuse further close/verify tool calls unless a new assignment arrives.
3. **Urgent stop honored** — Fixture: close task → send urgent stop → assert worker makes zero further `task close` / verification MCP calls.
4. **Outbox dedup** — Replayed kickoff/assignment messages after close should carry `redelivery: true` or be suppressed post-ack (extend BUG-stale-message-sequencing tests).

## Out of scope

- Ozer product code changed by cas-a651
- Merge-gate / awaiting_merge behavior (task closed successfully before the loop)

---

Reporting-only relay. No Cassy or Ozer source modified to produce this file.

---

## Resolution (cas-b269, 2026-07-11)

**Status:** Fixed on `factory/hv-director`.

### Characterization (pre-fix)

- `cas_task_close` had no early exit for `TaskStatus::Closed`, so stale re-close guidance re-entered the verification pipeline.
- `cas_verification_add` allowed recording verification against closed tasks.
- Urgent messages interrupted the turn but set no durable agent state, so MCP close/verify could continue after the interrupt.

### Fix (action-boundary revalidation — not merge-gate / product)

1. **`stale_close_guard` pure helpers** — terminal-closed check, already-closed close message, verification-on-closed error, urgent halt metadata, optional delivery rewrite for stale close/verify prose.
2. **`cas_task_close`** — if already `Closed`, return idempotent `ALREADY CLOSED` success (no re-verify). If agent `halt_task_work` is set, reject close.
3. **`cas_verification_add`** — reject when task is `Closed` or agent is halted.
4. **Urgent message** — sets `agent.metadata[halt_task_work]=1` on the target worker.
5. **`cas_task_start`** — clears `halt_task_work` so legitimate new assignments work.

Close-merge semantics unchanged. Unguarded product/Ozer code out of scope.

### Proof

```text
cargo test -p cas --lib -- test_b269
```

### Review follow-up (halt durability)

1. Halt cleared only after `task start` fully succeeds (InProgress persisted); failed starts preserve halt.
2. Urgent `all_workers` expands halt to every Worker-role agent.
3. Only supervisor/director sources may set halt; worker→supervisor never halts.
4. Halt persisted before queue enqueue (fail closed if persist fails).

### Review 2 (session scope + atomicity + generation)

1. Halt recipients scoped to `CAS_FACTORY_SESSION` (same-name cross-session excluded).
2. Agent-store open/list errors fail closed on urgent authorized halt.
3. Halt fan-out + enqueue all-or-none with compensating rollback on enqueue failure.
4. `halt_task_work_gen` generation; start clears only if gen <= start ceiling.
5. `AgentRole::Director` authorized explicitly via role.
