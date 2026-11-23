# Recovery — Rejections and Interrupted Work

Read for a rejection, blocked work, interruption or branch transition.

## MERGE REQUIRED

A successful park is not this refusal; MERGE REQUIRED means unpushed or
unattributable work.

1. Drain `coordination action=inbox_poll` to `No unread messages`, act on
   unread supervisor messages and re-read task state.
2. Capture `git rev-parse factory/<name>` (or the active per-task branch) and
   check `git merge-base --is-ancestor <delivered-tip> <target-tip>`; if it
   already landed, await the verdict rather than amend stale state.
3. For `local_merge`, keep the local commit and send
   `coordination action=message target=supervisor merge_request=true summary="Merge requested" message="<branch, SHA, target and evidence>"`;
   push only with explicit supervisor authorization. Otherwise push that branch
   and retry close once.
4. The supervisor owns the merge venue, so never open or merge a PR yourself.
   Fix what the exact error names; never bypass close with `status=closed` or
   an invented verification record.

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
into a parked delivery or a branch with an active completion receipt or review;
never rebase those reviewed tips. A pushed checkpoint on an owned InProgress
task is not a park, even when commit-time branch/anchor capture exists. After an
explicit supervisor sync request, checkpoint first, rebase onto its named target,
and push with `--force-with-lease=refs/heads/<branch>:<observed-checkpoint-sha>`.
H-40 checks task ownership, recorded start ancestry and delivery/review state;
unknown state refuses. A stale lease refuses rather than replacing a peer push. A fresh task whose checkout
still holds an earlier PR starts on `factory/<name>-<task>` from the target and
leaves the earlier branch alone.

### Returned delivery with a queued successor

When the supervisor returns delivery A while queued successor B is running:

1. Checkpoint B's changes as B's commits, then `task action=update id=B status=blocked`
   (this keeps B's queue slot).
2. Switch to A's branch from `task action=show id=A full=true`, run
   `task action=start id=A`, then correct and re-deliver A, merging its target.
3. After A closes or parks, switch to B's branch, run
   `task action=start id=B` directly. Do not insert `status=open`: an existing
   exact delivery proof locks that ordinary status edit.

## Blockers and review waits

Capacity refusals follow [Capacity waits](discipline.md#capacity-waits) instead.
When required supervisor review, approval or another decision blocks work:

1. Add a `note_type=blocker` note with the decision needed, the blocked next
   action, commit and proof; run `task action=update id=<task-id> status=blocked`.
2. Send `coordination action=message target=supervisor task_id=<task-id> blocker=true summary="Review required" message="..."`
   as the return contract with `status: blocked` and `blocker: <cause>`; omit `kind`.
3. Wait for the reply and record its decision. For approved work in the same
   scope, run `task action=start id=<task-id>` directly from Blocked; omit
   `status=open`. Start resumes the worker without replacing an existing exact
   delivery/verification proof. Keep supplemental findings in progress notes.
   If the approval requires changing reviewed scope or proof, ask the supervisor
   to recover that proof cycle before making those changes; workers cannot
   invalidate it. Mark model action you did not observe NOT VERIFIED.

## Verification-required close

The refusal says whether Cassy already delivered the dispatch; send only the
blocker it names, if any. To wait, set `status=blocked` with exactly this
blocker note, which stays quiet (any other text wakes the supervisor as a real
blocker):
`Verification required: exact dispatch <dispatch-id> must record a verdict before close.`
Start no other task meanwhile; after five minutes without a needed response,
read task state and send one follow-up.

## Universal denial or connectivity failure

If every MCP call is denied or fails, keep the error, inspect `.mcp.json` and the
`cas serve` process, and report it; a stale runtime may need a respawn. Never
edit SQLite or change identity or environment to bypass a denial. A
workspace-path denial means using the worktree, task artifacts directory or
harness scratchpad. A recurring "fixed" CAS bug is a runtime/source mismatch to
report before filing a duplicate.

## Interrupted assignment or checkout

- Reassignment or stand-down: checkpoint authorized work, push unless
  local_merge, note done and remaining work with the SHA, and stop editing.
- Low context: at 80% (Cassy's `context used: N%` notice; `unknown` is not
  exhaustion) or the harness compaction warning, commit, push, write
  `HANDOFF.md` (SHAs, remaining steps, next command, open questions) under the
  task artifacts directory printed in the assignment brief, never the repo
  root, then send one `coordination action=message target=supervisor kind=respawn summary=Checkpoint message="<return contract>" task_id=<task>`.
  Cassy recycles you with the same recipe and task bindings, and the
  replacement resumes from `HANDOFF.md`. A second request within ten minutes
  is held; a recycle safety refusal still requires a checkpoint.
- Conflict: checkpoint before rebasing onto the supervisor's named branch;
  resolve owned conflicts and escalate unowned ones with files and hashes.
- Failure outside your diff: run the same check on the base when permitted and
  report which side failed. Rust checks/tests follow [discipline.md](discipline.md).
- Missing submodule: inspect the main checkout's copy first; never silently
  change dependencies.
