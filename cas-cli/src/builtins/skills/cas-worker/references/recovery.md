# Recovery — Rejections and Interrupted Work

Read for a rejection, blocked work, interruption or branch transition.

## MERGE REQUIRED

A successful park is not this refusal. The park relay is the completion signal; send merge_request=true only for local_merge.

Drain `coordination action=inbox_poll` to `No unread messages`, follow
unread supervisor messages and re-read task state: the tip may have landed.
Capture `git rev-parse factory/<name>` (or the active per-task branch). For
`local_merge`, keep the local commit and send
`coordination action=message target=supervisor merge_request=true summary="Merge requested" message="<branch, SHA, target and evidence>"`.
Otherwise push that branch and retry close once. Never open or merge a PR
yourself (`gh pr create`). If rejected, fix what the exact error names; never
bypass close with `status=closed` or an invented verification record.

### While the merge is pending

Park releases your lease, and the merge verdict wakes you once: it closes the
task or reopens it with findings. Every poll spends your context.

- Wait on an event reminder, never a timed poll. For `local_merge`, one
  `branch_contained_in` reminder for the delivered SHA and target is that event.
- Check ancestry (`git merge-base --is-ancestor <tip> <target>`) once per
  supervisor message, never in a self-reminder loop.
- When the supervisor says it owns the merge, cancel reminders
  (`coordination action=remind_cancel`) and stay idle until it messages you.

## Queued successor and branch reuse

Cassy may already have started a queued successor on its task branch; continue
it. Checkpoint dirty work before changing history. Merge the target
into a branch with a recorded parked delivery anchor; never rebase it. Elsewhere,
checkpoint before an allowed `git rebase <target>`. A fresh task whose checkout
still holds an earlier PR starts on `factory/<name>-<task>` from the target and
leaves the earlier branch alone.

### Returned delivery with a queued successor

When the supervisor returns delivery A while queued successor B is running:

1. Checkpoint B's changes as B's commits, then `task action=update id=B status=blocked`
   (this keeps B's queue slot).
2. Switch to A's branch from `task action=show id=A full=true`, run
   `task action=start id=A`, then correct and re-deliver A, merging its target.
3. After A closes or parks, switch to B's branch, run
   `task action=update id=B status=open`, then `task action=start id=B`.

## Blockers and review waits

Capacity refusals follow [Capacity waits](discipline.md#capacity-waits) instead.
When required supervisor review, approval or another decision blocks work:

1. Add a `note_type=blocker` note with the decision needed, the blocked next
   action, commit and proof; run `task action=update id=<task-id> status=blocked`.
2. Send `coordination action=message target=supervisor task_id=<task-id> blocker=true summary="Review required" message="..."`
   as the return contract with `status: blocked` and `blocker: <cause>`; omit `kind`.
3. Wait for the reply and record its decision. Then `task action=update
   id=<task-id> status=open` and `task action=start id=<task-id>` before resuming.
   Mark model action you did not observe NOT VERIFIED.

## Verification-required close

When close returns VERIFICATION REQUIRED naming a dispatch:
Cassy closes eligible merged deliveries on the verdict; the supervisor owns any remaining close recovery, including no-code deliveries, and the worker never re-closes.
On older runtimes that queue a worker retry, the supervisor reads the exact
dispatch and retries ordinary close through the existing gates.
Send only the blocker the refusal
names, if any, set `status=blocked` and record exactly this one-line blocker note:
`Verification required: exact dispatch <dispatch-id> must record a verdict before close.`
Any other text wakes the supervisor as a real blocker. Start no other task
meanwhile; after five minutes without a needed response, read task state and
send one follow-up.

## Universal denial or connectivity failure

If every MCP call is denied or fails, keep the error, inspect `.mcp.json` and the
`cas serve` process, and report it; a stale runtime may need a respawn. Never
edit SQLite or change identity or environment to bypass a denial. A
workspace-path denial means using the worktree, task artifacts directory or
harness scratchpad. A recurring "fixed" CAS bug is a runtime/source mismatch to
report before filing a duplicate.

## ToolSearch resolved the tool but you still can't call it

Do not re-run ToolSearch for a tool it already resolved. Make a *separate* call to that tool by its full prefixed name and actual arguments. If that call
fails, report the error.

## Interrupted assignment or checkout

- Reassignment or stand-down: checkpoint authorized work, push unless
  local_merge, note done and remaining work with the SHA, and stop editing.
- Low context: At 80% or at the harness compaction warning: commit, push, write `HANDOFF.md` under the task artifacts directory printed in the assignment brief (never the repo root), send one `kind=status` with `need: respawn`.
  Read 80% from Cassy's `context used: N%` notice (`context used: unknown` is
  not exhaustion). `HANDOFF.md` goes in the task artifact directory (SHAs,
  remaining steps, next command, open questions); the respawned worker resumes from it.
- Conflict: checkpoint before rebasing onto the supervisor's named branch;
  resolve owned conflicts and escalate unowned ones with files and hashes.
- Failure outside your diff: run the same check on the base when permitted and
  report which side failed. Rust checks/tests follow [discipline.md](discipline.md).
  Every changed Rust crate needs a capped test receipt at the delivered tip; a runner denial is a blocker, not a deferral.
- Missing submodule: inspect the main checkout's copy first; never silently
  change dependencies.
