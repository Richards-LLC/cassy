# Reminder Discipline — Push First, One Bounded Checkpoint

Reminders cover quiet work and frozen-suite job checks, not acknowledgement, liveness, merge authority or task status. Start from lifecycle events and worker messages; on a reminder turn, read the authoritative task or worker state once, then act.

## Decision table

| Situation | First action | Reminder? |
|---|---|---|
| Worker reports `MERGE REQUIRED` | Merge now; Cassy runs the verifier on any pre-merge dispatch it opens | No; cancel any phase reminder after |
| Parked merge refused only for pending CI | Let Cassy's armed exact-SHA retry finish | No; act only on its notice |
| Ordinary CI on a pushed or integrated SHA | Record the SHA and end the turn | One `ci_completed` or `branch_contained_in` reminder, never a delay |
| Frozen train suite | Background the wait and act on the completion notice | No short timers; the daemon watchdog is the backstop |
| Long release, publication or detached job | Record its receipt and end the turn | One event reminder, or the harness background-task notice |
| Context-pressure handoff | Commit, push, task note, explicit handoff | One time checkpoint only if no next owner can acknowledge in time |
| Blocked worker recovery | Read state and send one recovery instruction | Only when an external dependency could otherwise go quiet |

Keep one active reminder per task or phase and cancel it when the phase completes or state changes. Never arm a timer to catch operator messages or inbox updates: Commander already wakes you, and a missing wake is a defect to file. Without any movement, the daemon's progress watchdog wakes you after 45 minutes.

## API contract

Every reminder has exactly one trigger, `remind_delay_secs` or `remind_event`. Use a delay only for work that becomes due at that time, such as an expiring handoff or a post-release invariant run, and pass `remind_ttl_secs` covering the phase. A task-filtered `task_completed` or `task_blocked` wait without TTL lasts until the task is terminal; a task-filtered deadline reached while the task is still live sends one owner notice. Keep the returned ID.

```text
coordination action=remind remind_delay_secs=300 remind_ttl_secs=900 \
  remind_message="Context handoff deadline: escalate to the next owner"
coordination action=remind_cancel remind_id=<reminder-id>
coordination action=remind remind_event=task_completed cross_session=true \
  remind_filter='{"task_id":"<task-id>"}' \
  remind_message="After this task completes, inspect its close/merge state once"
```

Without `remind_filter` an event fires on any task's completion. `cross_session=true` keeps a reminder across session end.

### Durable external checkpoints

Git conditions survive daemon and session restarts and need `cross_session=true` and a JSON filter. They expire after seven days without `remind_ttl_secs` (`0` means never), or once owner and target have been gone for 24 hours. `target_branch` is the bare origin branch; tags must be published to origin.

```text
coordination action=remind remind_event=branch_contained_in \
  remind_filter='{"commit":"<delivered-sha>","target_branch":"main"}' \
  cross_session=true remind_message="Delivery landed; inspect task and close"

coordination action=remind remind_event=tag_exists \
  remind_filter='{"tag":"v<release>"}' \
  cross_session=true remind_message="Release tag exists; inspect the authoritative receipt"

coordination action=remind remind_event=ci_completed \
  remind_filter='{"sha":"<full-40-char-sha>","workflow":"CI"}' \
  cross_session=true remind_message="CI finished on <sha>; merge or route the red job"
```

`ci_completed` fires once every run for that SHA (or the named workflow) completes, with each conclusion and URL; without `workflow`, scheduled runs are ignored. The daemon checks origin about once a minute and backs off failing refs up to 30 minutes.

## Workers

A worker records a detached command's log and next action in a progress note and uses its background-task notice. A capped build-admission refusal may use one timer to retry at its admission time. A context handoff is commit, push, task note and supervisor message.
