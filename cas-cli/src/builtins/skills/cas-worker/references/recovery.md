# Recovery — Rejections and Interrupted Work

Read for a rejection, blocked work, interruption or branch transition.

## MERGE REQUIRED

Drain `coordination action=inbox_poll` to `No unread messages`, follow
unread supervisor messages and re-read task state: the tip may have landed.
Capture `git rev-parse factory/<name>` (or the active per-task branch). For
`local_merge`, keep the local commit; otherwise push that branch. Send
`coordination action=message target=supervisor merge_request=true summary="Merge requested" message="<branch, SHA, target and evidence>"`.
Its verdict closes or reopens the task; do not re-close. For an epic, request
the merge rather than `gh pr create --base epic/...`. Forward a rejection's exact
error; never bypass close with `status=closed` or an invented verification record.

### While the merge is pending

The supervisor owns the landing signal; polls spend your context.

- Check ancestry (`git merge-base --is-ancestor <tip> <target>`) once per
  supervisor message, never in a self-reminder loop.
- Until the supervisor acknowledges the request, keep one `branch_contained_in`
  reminder for the delivered SHA and target, never a delay reminder.
- When it says it owns the merge or close, cancel those reminders
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

Workers close their own tasks. When close returns VERIFICATION REQUIRED naming a
dispatch, forward it once to the supervisor with the delivery SHA, set
`status=blocked`, record exactly this one-line blocker note, wait, and retry
close after the verdict:
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
- Low context: at 80% by Cassy's `context used: N%` notice (`context used:
  unknown` is not exhaustion), commit and push unless local_merge, write
  `HANDOFF.md` in the task artifact directory (SHAs, remaining steps, next
  command, open questions), cite it in a progress note and request respawn.
- Conflict: checkpoint before rebasing onto the supervisor's named branch;
  resolve owned conflicts and escalate unowned ones with files and hashes.
- Failure outside your diff: run the same check on the base when permitted and
  report which side failed.
- Missing submodule: inspect the main checkout's copy first; never silently
  change dependencies.
