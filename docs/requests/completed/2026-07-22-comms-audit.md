---
from: Ozer factory (supervisor cosmic-gopher-54, session ozer-ready-cardinal-20)
date: 2026-07-21
priority: P1
cas_task: cas-4fb4
---

# Cassy factory communication audit — 2026-07-21

The communication system was **not seamless** across the Ozer factories observed on 2026-07-21. Healthy normal-priority messages reached the target inbox in 25–101 ms, but other normal messages stalled for 18–24 minutes and drained only during shutdown. Urgent interrupt delivery was consistent at about 1.2 seconds. All four previously filed failure modes reproduced during the day; two additional observability defects made the persisted records contradict the actual logs.

The first four sections provide fresh same-day evidence for existing open tickets. They are not duplicate bug filings.

## 1. Normal-priority messages stall while workers are idle

**Existing ticket:** `cas-9599`

### Symptoms

Normal supervisor-to-worker and worker-to-supervisor messages sometimes remained pending while their targets were registered and idle. The messages were eventually processed together during factory shutdown. The urgent path continued to deliver promptly.

### Concrete evidence

All timestamps below are UTC.

- Message `3631`, supervisor → `telehealth-auditor`, was enqueued at `2026-07-21T13:22:48.002049256Z` with summary `Resume cas-0ef5 now — Tier 1 per msgs 3585 + 3626`.
- Message `3633`, supervisor → `telehealth-auditor`, was enqueued at `2026-07-21T13:24:24.925395203Z` with summary `Found the /home bug: shortcut intent is missing the widget's activity flags`.
- The morning supervisor queried both at 13:27:44:

  ```text
  2026-07-21T13:27:44.274Z  Message 3633 status: pending
  2026-07-21T13:27:44.580Z  Message 3631 status: pending
  ```

- `.cas/logs/cas-2026-07-21.log` records their eventual delivery:

  ```text
  2026-07-21T13:47:10.857819Z message_id=3631 deliver_ms=1462855
  2026-07-21T13:47:10.858437Z message_id=3633 deliver_ms=1365933
  ```

  That is 24m22.855s and 22m45.933s respectively, at the shutdown/queue-drain boundary.

- The reverse path also stalled. Worker completion message `3630` was enqueued at `13:22:28.677Z` and processed at `13:47:10.858Z` (24m42.181s). Worker resend `3639` was enqueued at `13:28:31.455Z` and processed at `13:47:10.968Z` (18m39.513s).
- This was intermittent rather than a blanket outage. Healthy normal messages `3518`, `3541`, `3548`, and `3609` delivered in 25 ms, 101 ms, 99 ms, and 50 ms.
- Urgent messages remained reliable:

  ```text
  message 3626: 13:18:52.459Z → 13:18:53.680Z (1.220s)
  message 3638: 13:28:15.156Z → 13:28:16.413Z (1.256s)
  ```

  Both were logged as `interrupt-and-redirect` / `urgent_interrupt`.

Four operationally important messages were delayed 18–24 minutes in the Jill wave alone. Later same-day Cassy records contain additional normal messages delayed until their factories shut down.

### Workaround applied

The supervisor queried `message_status`, switched urgent instructions to `coordination action=interrupt`, and later shut down the factory, which flushed the pending normal queue.

### Likely root cause (hypothesis)

Idle workers do not reliably poll or wake on normal prompt-queue inserts. A pending message therefore waits for a later lifecycle event or shutdown drain. The successful 25–101 ms examples suggest a wakeup/selection race, not a universally broken transport.

### Proposed fix

- **(a)** Make every normal enqueue actively wake a registered target and record the wake attempt/result.
- **(b)** Add a bounded delivery retry when a registered target remains unselected beyond a small threshold.
- **(c)** Add a supervisor-visible stalled-delivery event after, for example, 5 seconds instead of requiring manual `message_status` checks.

Lean recommendation: **(a) plus (c)**. A reliable wake fixes the behavior; an explicit stalled-delivery event keeps future regressions visible.

---

## 2. `MERGE REQUIRED` forces the state rejected by the zero-commit gate

**Existing ticket:** `cas-6325`

### Symptoms

A worker close correctly transitions a code task to `awaiting_merge`. After the supervisor merges the worker branch into the epic branch, the required worker re-close sees zero unique commits and is rejected as a zero-commit code task. The supervisor must bypass-close work that is already merged and reviewed.

### Concrete evidence

- `cas-a889` task notes:

  ```text
  2026-07-21 12:42 Close rejected: MERGE REQUIRED. Task parked as awaiting_merge.
  2026-07-21 12:46 Closed: Supervisor close after independent verification.
                     ZERO-COMMIT gate false positive after e7e354bc was merged.
  ```

- `cas-8b07` task notes and supervisor transcript:

  ```text
  2026-07-21 12:50 Close rejected: MERGE REQUIRED. Task parked as awaiting_merge.
  2026-07-21T12:51:24.988Z worker: cas-8b07 hit the zero-commit false positive
  2026-07-21 12:51 supervisor close after ae042b26 was merged.
  ```

- The same-day `cas-6325` record lists the same reproduction on `cas-e114`, `cas-5d7a`, `cas-7f61`, and `cas-0ef5`.
- Six code-bearing Jill-wave tasks required a supervisor bypass-close. The investigation-only `cas-3d23` also hit an inappropriate zero-commit assumption.
- Push-based merge signaling itself worked. Director injected actionable source branch, merge target, and re-close instructions for `cas-a889` at `12:46:05.545Z` and `cas-7f61` at `13:06:55.972Z`. The failure was the prescribed re-close path.

### Workaround applied

The supervisor verified commit ancestry and proof notes, merged each branch, then closed with the supervisor bypass and a written justification.

### Likely root cause (hypothesis)

The close gate computes only commits currently unique to the worker branch. Once the supervisor merges those commits into the parent, the correct result becomes zero, but the gate interprets that expected post-merge state as evidence that no work existed.

### Proposed fix

- **(a)** Persist the worker commit SHA set when `MERGE REQUIRED` is emitted, then accept re-close when those SHAs are ancestors of the parent branch.
- **(b)** Use task-linked commit provenance rather than current branch uniqueness.
- **(c)** Let a verified `awaiting_merge → merged` transition satisfy the code-change requirement without a second branch-diff inference.

Lean recommendation: **(a)** as the smallest auditable fix, with **(b)** as the longer-term model.

---

## 3. Multi-epic coordination is not consistently factory-scoped

**Existing ticket:** `cas-2cf9`

### Symptoms

Assignment freshness checks, reminder triggers, shared-worktree placement, and worker-to-supervisor routing crossed active epic/factory boundaries.

### Concrete evidence

- Assigning Jill task `cas-3d23` to `telehealth-auditor` failed at `2026-07-21T12:46:27.002Z`:

  ```text
  Cannot assign to worker 'telehealth-auditor': 10 commits behind
  epic/widget-parity-saffron-light-mode-jill-request-cas-960d (threshold: 1)
  ```

  `cas-3d23` belongs to Jill epic `cas-60e3`, not widget-parity epic `cas-960d`. `focus_epic(cas-60e3)` was accepted at `12:46:45`; the retry failed identically at `12:46:48.034Z`.

- Reminder `#27` belonged to the sleep-rhythm supervisor and had `trigger_event=task_completed` with no task/epic filter. It fired at `12:52:40.878Z` on Jill task `cas-3d23`:

  ```text
  fired_event task_id=cas-3d23, worker=telehealth-auditor
  message: Sleep-rhythm task completed — run the review gate ...
  ```

- A non-isolated `onepass-info-builder` started in the shared main directory on an unrelated sleep-rhythm epic branch. Supervisor message `3518` warned about the collision at `12:36:55.718Z`; the worker reported at `12:42:23.975Z` that its Jill commit had landed on that wrong branch and had to be relocated.
- Worker messages from `recipes-fixer-2` in factory session `ozer-strong-jay-96`, including ACK `3551` and evidence resend `3639`, targeted `warm-falcon-13` instead of the Jill supervisor and drained much later. This compounded the normal-queue delay with wrong-supervisor routing.

### Workaround applied

The supervisor avoided the suggested unrelated rebase, reset/reassigned tasks manually, moved work into isolated worktrees, recovered the misplaced commit, and treated cross-factory reminders as noise after checking task ownership.

### Likely root cause (hypothesis)

Some coordination paths resolve context from a repo-global current epic or canonical `supervisor` name rather than the target task's parent epic and factory-session ownership. Reminder filters match event type globally when no task filter is supplied.

### Proposed fix

- **(a)** Resolve assignment gates from the target task's parent epic exclusively.
- **(b)** Require `factory_session` and/or epic scope on reminders and worker reply routing; reject ambiguous creation.
- **(c)** Refuse non-isolated spawns when the shared checkout is attached to another active factory/epic.

Lean recommendation: implement **all three**; they guard separate mutation, event, and filesystem boundaries.

---

## 4. `spawn_workers task_id` preassignment is skipped or orphaned

**Existing ticket:** `cas-0885`

### Symptoms

On the Codex isolated path, `spawn_workers` promised preassignment but the worker booted with no assigned task. On the Claude path, preassignment occurred but survived shutdown of never-started workers, leaving ghost ownership without an active lease.

### Concrete evidence

- Codex spawn request `389` returned at `2026-07-21T12:44:16.123Z`:

  ```text
  Task: cas-7f61 will be pre-assigned once the worker boots
  ```

- Director injected `recipes-fixer-2 is ready and waiting for tasks` at `12:44:17.930Z`, then `idle with no assigned tasks` at `12:44:48.359Z`.
- Supervisor sent manual assignment message `3548` at `12:44:59.435Z`; lease history shows the real claim at `12:45:06`.
- Request `387` reproduced the Codex-side failure. Two Codex spawns therefore required manual assignment.
- Inverse Claude-path evidence: `cas-3d23` remained assigned to shut-down `sow-auditor`. The replacement worker's self-start failed at `12:47:35.845Z`; transfer failed `No active lease found` at `12:47:44.969Z`; `reset` at `12:47:47.503Z` was required. `cas-5d7a` had the same orphan pattern.

### Workaround applied

The supervisor manually messaged Codex workers with the task ID and used `reset` to clear Claude-path ghost ownership before a new worker could start.

### Likely root cause (hypothesis)

Codex boot/worktree provisioning races or bypasses the preassignment callback. Conversely, shutdown cleanup releases worker processes and leases without atomically clearing never-started task ownership/status.

### Proposed fix

- **(a)** Make spawn completion transactional: do not report the worker ready until requested task assignment is committed and visible to `action=mine`.
- **(b)** On shutdown, atomically reset preassigned-but-never-started tasks to open/unassigned.
- **(c)** Emit a failed-spawn/failed-preassignment event instead of a false success acknowledgement.

Lean recommendation: **(a) and (b)**, with **(c)** as the safety signal.

---

## 5. `message_status` historical hydration is self-contradictory

**New bug.**

### Symptoms

For historical messages that the Cassy text log proves were delivered, `message_status` reports `legacy_status: Delivered` while simultaneously reporting authoritative `stage: enqueued`, `delivered_at: null`, and `pending_reason: awaiting_delivery`.

### Concrete evidence

Fresh `message_status` calls on messages `3518`, `3541`, `3548`, `3609`, `3626`, `3631`, `3633`, `3638`, and `3639` returned this contradictory shape. Example for `3631`:

```json
{
  "id": 3631,
  "legacy_status": "Delivered",
  "stage": "enqueued",
  "enqueued_at": "2026-07-21T13:22:48.002049256Z",
  "selected_at": null,
  "delivered_at": null,
  "pending_reason": "awaiting_delivery"
}
```

The text log independently records `3631` delivered at `13:47:10.857819Z` with `deliver_ms=1462855`. Recent messages created after the new telemetry path, such as `3704` and `3706`, correctly show `stage: delivered` and populated `selected_at`/`delivered_at`.

### Workaround applied

The audit used `.cas/logs/cas-2026-07-21.log` as the authoritative delivery source and treated the historical `message_status` stage fields as unavailable.

### Likely root cause (hypothesis)

The new lifecycle columns were introduced without backfilling historical `processed_at`/transport delivery evidence. The status formatter mixes a legacy-derived top-line label with unhydrated new-stage fields.

### Proposed fix

- **(a)** Backfill `selected_at`, `transport_delivered_at`, and `highest_stage` from trustworthy historical queue/log fields.
- **(b)** If backfill is unsafe, return `stage: legacy_delivered` and explicitly mark lifecycle timestamps unavailable.
- **(c)** Never show the legacy summary as `Delivered` when the detailed stage says `enqueued`; choose one internally consistent representation.

Lean recommendation: **(b) immediately**, followed by **(a)** where deterministic evidence exists.

---

## 6. `lease_history` mislabels awaiting-merge releases as task closure

**New bug.**

### Symptoms

Lease history says the worker lease was released `from Task closed` at the moment a close was rejected and the task moved to `awaiting_merge`.

### Concrete evidence

- `lease_history(task_id=cas-a889)` reports:

  ```text
  [2026-07-21 12:42:13] released ... (from Task closed)
  ```

  The task note at the same minute says `Close rejected: MERGE REQUIRED. Task parked as awaiting_merge`; actual closure occurred at 12:46.

- `lease_history(task_id=cas-8b07)` reports:

  ```text
  [2026-07-21 12:50:00] released ... (from Task closed)
  ```

  The task note says `Close rejected: MERGE REQUIRED`; actual supervisor closure occurred at 12:51.

These are two direct reproductions in the same factory wave.

### Workaround applied

The audit cross-referenced task notes and the supervisor transcript rather than trusting the lease-history reason label.

### Likely root cause (hypothesis)

The close handler releases the lease before the merge-state rejection is finalized, and the lease audit receives a generic `Task closed` reason instead of the resulting `awaiting_merge` transition.

### Proposed fix

- **(a)** Pass the final transition reason (`awaiting_merge`, `closed`, `verification_required`, and so on) into lease release logging.
- **(b)** Delay lease release audit insertion until the close outcome is known.
- **(c)** Add task status before/after fields to lease-history events so consumers need not infer state from prose.

Lean recommendation: **(c) plus (a)**. Structured state is durable; the human-readable reason should match it.

---

## Observability gap: no authoritative ACK or reaction measurement

The message lifecycle records enqueue and transport delivery, but the audited records have no authoritative `confirmed_at`, wake, or reaction timestamps. A worker's human-readable `ACK` is a separate normal message and may itself stall or route to the wrong supervisor. Consequently, send → reaction latency cannot be derived reliably even when transport delivery is known.

Recommended direction: have the harness automatically call `message_ack` when an injected turn is accepted, and separately record the first task/tool action causally associated with the message. Keep transport delivery, receipt ACK, and behavioral reaction as distinct stages.

## Summary for triage

| # | Issue | Severity | Cost today | Affects |
|---|---|---|---|---|
| 1 | Normal messages stall until shutdown (`cas-9599`) | P1 | Four critical Jill-wave messages delayed 18–24 min; additional same-day stalls | Supervisors and workers, both directions |
| 2 | Merge-required → zero-commit catch-22 (`cas-6325`) | P1 | Six code tasks required supervisor bypass-close; one no-code task also misfired | Every code-bearing epic child close |
| 3 | Multi-epic scoping leaks (`cas-2cf9`) | P1 | Wrong-epic assignment blocked twice; one commit relocated; reminder and reply routing crossed factories | Concurrent factories in one repo |
| 4 | Spawn preassignment skipped/orphaned (`cas-0885`) | P1 | Two Codex spawns needed manual assignment; two Claude tasks needed ghost-owner recovery | Spawn, shutdown, task ownership |
| 5 | Historical `message_status` contradiction | P2 | Nine sampled messages required raw-log correlation; delivery API could not be trusted | Audits, monitoring, incident response |
| 6 | `lease_history` reports rejected close as closed | P2 | Two sampled merge transitions required task-note correlation | Merge automation and forensic audits |
| 7 | No ACK/reaction instrumentation | P2 observability | Send → reaction latency underivable; content ACKs can themselves stall | Delivery SLOs and supervisor automation |

The Ozer team can provide further excerpts or message/task records if useful. Primary evidence remains available at:

- `/home/pippenz/.claude/projects/-home-pippenz-Petrastella-ozer/da47f5ca-ff49-4c8f-986a-8d16f887b961.jsonl`
- `/home/pippenz/.claude/projects/-home-pippenz-Petrastella-ozer/0665ad66-c0f3-41a5-b1b5-fdce26636396.jsonl`
- `/home/pippenz/Petrastella/ozer/.cas/logs/cas-2026-07-21.log`


## Completion

- **completed:** 2026-07-22 (items 5, 6; partial 7)
- **resolution:** Items 1–4 track existing Ozer tickets; their fixes shipped the same day in epics cas-887b (zero-commit catch-22, assign-gate epic scoping, spawn preassign + shutdown release) and cas-873a (real-time delivery, head-of-line, urgent framing) — the audited factory ran a pre-fix binary. Re-verify cas-9599 delivery latency on v2.28.1 before further work. Item 5 (message_status self-contradiction) fixed in v2.28.1: one-time gated backfill of `highest_stage`/`transport_delivered_at` from `processed_at`; live legacy paths are never promoted. Item 6 (lease_history mislabel) fixed in v2.28.1: release reasons threaded through every `release_lease_for_task` call site. Item 7 (ACK/reaction instrumentation): stage/wake/reaction fields now exist in `message_status`; automatic harness `message_ack` remains open — follow-on cas-7aef covers the remaining lease-history field wart.
