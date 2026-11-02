# Reminder Discipline — Push First, One Bounded Checkpoint

Reminders are a safety net for work that may otherwise go quiet. They are not
polling, acknowledgement, liveness, merge authority, or task status. Start from
push lifecycle events and worker messages; on a reminder turn, read the
authoritative task/worker state once before acting.

## Decision table

| Situation | First action | Reminder? | Advance/cleanup |
|---|---|---|---|
| Worker reports `MERGE REQUIRED` | Drain the merge queue immediately | No — the push is actionable | Merge; if Cassy opened a dispatch, run the verifier on it (its verdict closes or returns the task; no worker message); a Tier 1 delivery in `qa.mode=batch` waits for the release verdict. Then cancel any phase reminder |
| GitHub CI on a pushed SHA, or a merge-queue landing | Record the SHA and end the turn | One `ci_completed` (CI) or `branch_contained_in` (landing) reminder — never a delay | Act once on the fired conclusions or landed SHA |
| Long release, publication, or other detached job | Record its receipt and end the turn | One time checkpoint, bounded 2–5 minutes | Check the authoritative job/task state once; cancel on early completion or replace the old reminder with one later checkpoint |
| Context-pressure handoff | Commit/push, task note, and explicit supervisor handoff | One time checkpoint only if no next owner can acknowledge before the handoff expires | Cancel when the new owner acknowledges or the task advances |
| Blocked worker recovery | Read task/heartbeat/process state and send one recovery instruction | Only after an external dependency or unreachable owner could otherwise go quiet | Cancel as soon as the worker responds, is reassigned, or the task closes |
| Concrete lifecycle event is already the desired trigger | Use the matching event, not a timer | One task-filtered event reminder, lasting until task terminal | Cancel it if the phase completes through another path |

Never create a reminder instead of checking authoritative task acceptance,
reading inbox updates, or processing an authenticated typed lifecycle event. Do not sleep,
watch CI, or repeatedly poll. One active reminder per task/phase is the limit.

## API contract

Every reminder has exactly **one** trigger: `remind_delay_secs` *or*
`remind_event`, never both. For time checkpoints, pass `remind_ttl_secs`
covering the phase. For `task_completed` or `task_blocked` with a task filter,
omit TTL to wait until the task is terminal; use an explicit positive TTL only
for a real deadline. A deadline reached while the task is still live sends one
visible owner notice. Keep the returned reminder ID, list only to reconcile a
known phase, and cancel or supersede it when state changes. Closing/merging a
task must leave no stale phase reminder.

### Time checkpoint: release or detached command

```text
coordination action=remind remind_delay_secs=300 remind_ttl_secs=900 \
  remind_message="Release <name>: inspect the authoritative job receipt and task state"
```

The reminder is self-targeted by default. If the release finishes early, clean
it up rather than letting it fire:

```text
coordination action=remind_cancel remind_id=<reminder-id>
```

If it remains in flight, cancel the old checkpoint first, then create one new,
later bounded time reminder — do not accumulate timers.

### Event checkpoint: only for a concrete lifecycle event

```text
coordination action=remind remind_event=task_completed cross_session=true \
  remind_filter='{"task_id":"<task-id>"}' \
  remind_message="After this task completes, inspect its close/merge state once"
```

A task-filtered wait reconciles persisted task status, including a close synced
from another machine. Use `cross_session=true` to retain it across session end.
Without `remind_filter` the reminder fires on any task's completion. `task_completed` is a real event; vague hopes such as "when CI is ready" are
not. For a parked delivery whose completion is an externally observable git
condition, use the durable external triggers below instead of a finite timer.

### Durable external checkpoint: branch or tag

External conditions survive daemon/session restart. When `remind_ttl_secs` is
omitted they expire after seven days (`0` means no expiry), and they also
expire once both their owner and target have had no heartbeat or live process
for 24 hours. They require `cross_session=true` and a JSON filter.
`target_branch` is the bare origin branch; `origin/main` is stored as `main`.
`branch_contained_in` also accepts `sha` for `commit`; other filter keys are
refused, and one refusal lists every missing requirement:

```text
coordination action=remind remind_event=branch_contained_in \
  remind_filter='{"commit":"<delivered-sha>","target_branch":"main"}' \
  cross_session=true remind_message="Delivery landed; inspect task and close"

coordination action=remind remind_event=tag_exists \
  remind_filter='{"tag":"v<release>"}' \
  cross_session=true remind_message="Release tag exists; inspect the authoritative receipt"
```

For CI, wait on the runs themselves instead of a delay reminder:

```text
coordination action=remind remind_event=ci_completed \
  remind_filter='{"sha":"<full-40-char-sha>","workflow":"CI"}' \
  cross_session=true remind_message="CI finished on <sha>; merge or route the red job"
```

It fires within a minute once every run for that SHA (or only the named
workflow) has completed, carrying each run's conclusion and URL and an
overall `outcome`. Without `workflow`, scheduled runs are ignored. For a
merge-queue landing, use `branch_contained_in` with `"target_branch":"main"`.

The daemon checks origin on a bounded one-minute cadence, off its input loop,
fetching each target branch once per check. A local-only tag cannot satisfy
`tag_exists`; the tag must be published to origin. A failed remote check
leaves the reminder pending and backs that ref off, up to 30 minutes between
checks. The fired description includes the
remote ref and SHA, and the reminder fires once when the condition becomes
true, delivering the existing supervisor notification and prompt-queue wake.

## Role patterns

- **Supervisors:** after dispatch, end the turn. For a long official release or
  merge/publication queue, schedule one bounded checkpoint only when there is
  no push expected before the next decision. On firing, inspect the official
  receipt and task/worker state once, then merge, release, reassign, or set one
  superseding checkpoint. Never retain reminders after merge or close.
- **Workers:** before ending a turn for a detached command, persist the command
  receipt/log path and next action in a progress note, message the supervisor
  if it needs a decision, then create one bounded self-reminder. On firing,
  read the receipt once and explicitly hand off the result; do not turn it into
  a poll loop. Context handoff is commit + push + task note + supervisor
  message first; the reminder only protects a quiet external follow-up.

The model pattern: one 2–5 minute checkpoint after an unreachable external
workflow, followed by a single authoritative check and state advance — not a
stream of stale reminders.

## Peer release updates

Send a peer merge/close update that unblocks their release with `kind=status`, never `kind=receipt`: receipts stay quiet. Treat `EPIC READY: all children closed` as an action to inspect the epic and advance its release.
