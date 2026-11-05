# Recovery — Rejections and Interrupted Work

Read for a rejection, blocked work, interruption or branch transition.

- [Merge required](#merge-required)
- [Queued successor and branch reuse](#queued-successor-and-branch-reuse)
- [Blockers and review waits](#blockers-and-review-waits)
- [Connectivity failure](#universal-denial-or-connectivity-failure)
- [Tool loading](#toolsearch-resolved-the-tool-but-you-still-cant-call-it)
- [Interrupted assignment or checkout](#interrupted-assignment-or-checkout)

## MERGE REQUIRED

A successful park is not this refusal. The park relay is the completion signal; send merge_request=true only for local_merge.

Drain `coordination action=inbox_poll` to `No unread messages`; follow
unread supervisor messages, re-read task state and confirm whether the tip already
landed before making a corrective commit.
Capture `git rev-parse factory/<name>` (or the active per-task branch).

For `local_merge`, keep the local commit and send
`coordination action=message target=supervisor merge_request=true summary="Merge requested" message="<branch, SHA, target and evidence>"`.
Otherwise push that branch and retry close once. Never open or merge a PR
yourself (`gh pr create`).
A squash/cherry-pick mismatch may need `commit_receipt`; transactional
`completion_receipt` accepts an unpublished source tip and binds its merge candidate
to the same required verification before publication. See [close-gate.md](close-gate.md)
for receipt fields. If rejected, fix what the exact error names;
never bypass close with `status=closed` or an invented verification record.

## Queued successor and branch reuse

A queued successor may already be started by Cassy on its task branch. Inspect
its task and branch, then continue that assignment. Checkpoint dirty work before
changing history. If a task branch has a recorded parked delivery anchor, merge
the target into it; never rebase it. On other reused checkouts where rebasing is
allowed, checkpoint before `git rebase <target>`.

A fresh task whose checkout still holds an earlier PR (open, or squash-merged
so its commits never reach the target) starts on `factory/<name>-<task>` from
the target. Push that branch for its own PR; leave the earlier branch alone.

### Returned delivery with a queued successor

When the supervisor returns delivery A while queued successor B is running:

1. Checkpoint B's unfinished changes as B's commits.
2. Run `task action=update id=B status=blocked`; this releases its lease while
   preserving its current queue slot.
3. Find A's recorded delivery branch with `task action=show id=A full=true`.
   Switch to that branch, run `task action=start id=A`, then correct and
   re-deliver A. Preserve its parked ancestry by merging its target.
4. After A closes or parks, switch to B's recorded branch, run
   `task action=update id=B status=open`, then `task action=start id=B`.

Pending work remains behind held B. The start refusal names the exact hold
command when another queue task is active.

### While the merge is pending

Park releases your lease, and the merge verdict wakes you once: it closes the
task or reopens it with findings. Every poll spends your context.

- Wait on an event reminder, never a timed poll. For `local_merge`, one
  `branch_contained_in` reminder for the delivered SHA and target is that event.
- Check ancestry (`git merge-base --is-ancestor <tip> <target>`) once per
  supervisor message you receive, never in a self-reminder loop.
- When the supervisor says it owns the merge, cancel reminders
  (`coordination action=remind_cancel`) and stay idle until it messages you.

## Blockers and review waits

For build-admission capacity refusals, use [Capacity waits](discipline.md#capacity-waits)
before this sequence.

When required supervisor review or approval blocks work (e.g. publication),
enter awaiting supervisor review:

1. Add a `note_type=blocker` task note with the required decision, blocked next
   action, commit and proof; run `task action=update id=<task-id> status=blocked`.
2. Send `coordination action=message target=supervisor task_id=<task-id> blocker=true summary="Review required" message="..."`
   using the return contract with `status: blocked` and `blocker: <cause>`.
   Omit `kind`: `kind=status` cannot accompany `blocker=true`.
3. Save the notification ID; wait for the review reply. Record its ID and actual
   decision/action. Clear the review hold with
   `task action=update id=<task-id> status=open`, then
   `task action=start id=<task-id>` before resuming.
   Track transport delivery and inbox receipt separately with `message_status`.
   Mark unobserved model action NOT VERIFIED; the supervisor owns that proof.

Other blockers use the same sequence. Finished deliveries park instead.
Wake eligibility never guarantees a turn.

## Universal denial or connectivity failure

If all MCP calls are denied or fail, preserve the error and inspect `.mcp.json`
and the `cas serve` process. Report to the supervisor through available
coordination; a stale runtime may need rebuild/respawn. Do not edit SQLite or
change identity/environment to bypass a denial.
For workspace-path denial, use the worktree for source/check output, configured
task artifacts for durable proof and harness scratchpad for ephemeral notes.
A `/dev/null` denial is a guard defect to report, not a reason to retry that path.
For a supposedly fixed CAS bug recurring, report the observed runtime/source
mismatch before creating a duplicate fix task.

## ToolSearch resolved the tool but you still can't call it

Do not re-run ToolSearch for a tool it already resolved. Make a *separate* call to that tool by its full prefixed name and actual arguments. If the call itself
fails, report that error rather than repeating schema lookup.

## Interrupted assignment or checkout

- Reassignment or stand-down: checkpoint authorized WIP, push unless local_merge,
  note completed/remaining work and send its SHA; stop edits on that task.
- Low context: At 80% or at the harness compaction warning: commit, push, write HANDOFF.md, send one `kind=status` with `need: respawn`.
  Read 80% from Cassy's `context used: N%` notice; never infer occupancy from
  billing tokens, tool-output size or a guessed model window. `HANDOFF.md` goes
  in the task artifact directory: done work with SHAs, remaining steps, the next
  command and open questions; the respawned worker resumes from it. Queued tasks
  keep their context unless `factory.restart_worker_between_tasks=true`; do not
  request an early recycle just because a prior task was long.
- Dependency/conflict failure: compare manifests and target state against the
  supervisor's named branch. Checkpoint before rebasing. Resolve owned conflicts;
  escalate unowned ones with affected files and hashes.
- Failure outside your diff: compare the same check on the base when permitted;
  report exact output and which side failed. Rust checks/tests follow
  [discipline.md](discipline.md). Every changed Rust crate needs a capped test receipt at the delivered tip; a runner denial is a blocker, not a deferral.
- Missing submodule: inspect the main checkout's initialized copy before choosing
  the project-supported linking/init route. Do not silently change dependencies.
