# Reminder Discipline — Push First, One Bounded Checkpoint

Reminders are a safety net for work that may otherwise go quiet. They are not
polling, acknowledgement, liveness, merge authority, or task status. Start from
push lifecycle events and worker messages; on a reminder turn, read the
authoritative task/worker state once before acting.

## Decision table

| Situation | First action | Reminder? | Advance/cleanup |
|---|---|---|---|
| Worker reports `MERGE REQUIRED` | Drain the merge queue immediately | No — the push is actionable | Merge, tell worker to re-close, then cancel any phase reminder |
| GitHub CI run or merge-queue landing | Record the SHA or PR and end the turn | One `ci_completed` or `merge_queue_landed` event reminder — never a delay | It fires within 60 s of completion with the conclusions; act on them once |
| Long release, publication, or other detached job with no GitHub event | Record its receipt and end the turn | One time checkpoint, bounded 2–5 minutes | Check the authoritative job/task state once; cancel on early completion or replace the old reminder with one later checkpoint |
| Context-pressure handoff | Commit/push, task note, and explicit supervisor handoff | One time checkpoint only if no next owner can acknowledge before the handoff expires | Cancel when the new owner acknowledges or the task advances |
| Blocked worker recovery | Read task/heartbeat/process state and send one recovery instruction | Only after an external dependency or unreachable owner could otherwise go quiet | Cancel as soon as the worker responds, is reassigned, or the task closes |
| Concrete lifecycle event is already the desired trigger | Use the matching event, not a timer | One event reminder with a TTL | Cancel it if the phase completes through another path |

Never create a reminder instead of checking authoritative task acceptance,
reading inbox updates, or processing an authenticated typed lifecycle event. Do not sleep,
watch CI, or repeatedly poll. One active reminder per task/phase is the limit.

## API contract

Every reminder has exactly **one** trigger: `remind_delay_secs` *or*
`remind_event`, never both. Always pass `remind_ttl_secs`; use the shortest TTL
that covers the phase. Keep the returned reminder ID, list only to reconcile a
known phase, and cancel or supersede it when state changes. Closing/merging a
task must leave no stale phase reminder.

### Time checkpoint: release or detached command

```
coordination action=remind remind_delay_secs=300 remind_ttl_secs=900 \
  remind_message="Release <name>: inspect the authoritative job receipt and task state"
```

The reminder is self-targeted by default. If the release finishes early, clean
it up rather than letting it fire:

```
coordination action=remind_cancel remind_id=<reminder-id>
```

If it remains in flight, cancel the old checkpoint first, then create one new,
later bounded time reminder — do not accumulate timers.

### Event checkpoint: only for a concrete lifecycle event

```
coordination action=remind remind_event=task_completed remind_ttl_secs=1800 \
  remind_filter='{"task_id":"<task-id>"}' \
  remind_message="After this task completes, inspect its close/merge state once"
```

Without `remind_filter` the reminder fires on any task's completion. `task_completed` is a real event; a vague hope such as "when CI is ready"
is not — use `ci_completed` below. For a parked delivery whose completion is
an externally observable git or GitHub condition, use the durable external
triggers below instead of a finite timer.

### Durable external checkpoint: branch or tag

External conditions survive daemon/session restart and do not expire when
`remind_ttl_secs` is omitted (`0` means no expiry). They require
`cross_session=true` and a JSON filter:

```
coordination action=remind remind_event=branch_contained_in \
  remind_filter='{"commit":"<delivered-sha>","target_branch":"main"}' \
  cross_session=true remind_message="Delivery landed; inspect task and close"

coordination action=remind remind_event=tag_exists \
  remind_filter='{"tag":"v<release>"}' \
  cross_session=true remind_message="Release tag exists; inspect the authoritative receipt"
```

### Durable external checkpoint: CI run or merge-queue landing

A delay reminder over CI wakes you long after the run finished. Wait on the
run itself:

```
coordination action=remind remind_event=ci_completed \
  remind_filter='{"sha":"<pushed-sha>","workflow":"CI"}' \
  cross_session=true remind_message="CI finished on <sha>; merge or route the red job"

coordination action=remind remind_event=merge_queue_landed \
  remind_filter='{"pr":<number>}' \
  cross_session=true remind_message="PR landed; close the task and continue the release"
```

`ci_completed` takes `sha` (7–40 hex; use the full SHA for a commit that
exists only on GitHub, such as a merge-queue commit) or `branch` (follows the
branch's forge tip), plus optional `workflow` (name such as `CI`, or file such
as `ci.yml`) and `event` (`push`, `pull_request`, `merge_group`). It fires once
every matching run for that commit has completed; without `workflow` or
`event`, scheduled runs are ignored. The fired event carries `outcome`
(`success`/`failure`) and each run's conclusion and URL, so read it instead
of re-querying GitHub. `merge_queue_landed` fires when the PR merges (with the
merge commit) or is closed unmerged; an ejected PR stays open and arrives as
the existing ejection wake. The daemon probes GitHub every 30 s while such a
reminder is pending, off its loop.

The daemon checks origin for branch and tag conditions on a bounded one-minute cadence. A local-only tag
cannot satisfy `tag_exists`; the tag must be published to origin. A failed
remote check leaves the reminder pending. The fired description includes the
remote ref and SHA, and the reminder fires once when the condition becomes
true, delivering the existing supervisor notification and prompt-queue wake.

## Role patterns

- **Supervisors:** after dispatch, end the turn. For CI on a pushed SHA or a
  PR in the merge queue, set one `ci_completed` / `merge_queue_landed`
  reminder. For any other long official release or publication step, schedule
  one bounded checkpoint only when there is no push expected before the next
  decision. On firing, inspect the official
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
