# Recovery — Rejections and Interrupted Work

Read when a tool rejects delivery, connectivity fails or assignment changes.

## MERGE REQUIRED

Drain `coordination action=inbox_poll` to `No unread messages`; follow
unread supervisor messages, re-read task state and confirm whether the tip already
landed before sending a merge request or making a corrective commit.
Capture `git rev-parse factory/<name>` (or the active per-task branch).

For `local_merge`, keep the local commit. Otherwise push that branch. Send
`coordination action=message target=supervisor merge_request=true summary="Merge requested" message="<branch, SHA, target and evidence>"`.
Do not re-close after the merge: its verdict closes the task, or reopens it to
you with findings. For an epic,
request supervisor merge rather than `gh pr create --base epic/...`.
A squash/cherry-pick mismatch may need `commit_receipt`; transactional
`completion_receipt` requires the source tip merged. See [close-gate.md](close-gate.md)
for receipt fields. If rejected, forward the exact error and receipt identity;
never bypass close with `status=closed` or an invented verification record.

## Queued successor and branch reuse

A queued successor may already be started by Cassy on its task branch. Inspect
its task and branch, then continue that assignment. Checkpoint dirty work before
changing history. If a task branch has a recorded parked delivery anchor, merge
the target into it; never rebase it. On other reused checkouts where rebasing is
allowed, checkpoint before `git rebase <target>`.

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

The supervisor owns the landing signal. Queue attempts take minutes, it sees
landing and ejection events you cannot, and every poll spends your context,
which is scarcest late in a lane.

- Check ancestry (`git merge-base --is-ancestor <tip> <target>`) once per
  supervisor message you receive, never in a self-reminder loop.
- Set a reminder for that check only while the supervisor has not acknowledged
  your merge request, and never sooner than 300 seconds.
- When the supervisor says it owns the merge or the close, or asks you to stop
  polling, cancel those reminders (`coordination action=remind_cancel`) and
  stay idle until it messages you.

## Verification-required close

Forward the exact task-scoped guidance once with delivery SHA and proof ownership.
This hands the task to the supervisor; start no other task until that handoff is
recorded. Verification waits do not authorize self-dispatch. Stay available for
assigned work. Before resending a delayed or replayed outbox message, read task
state: it may already be closed. After five minutes without a needed response,
read state and send one follow-up, rather than polling or repeating idle notices.

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
- Low context: at about 80% context used, commit and push unless local_merge,
  then write `HANDOFF.md` in the task artifact directory: done work with SHAs,
  remaining steps, the next command and open questions. Cite it in a progress
  note and request respawn; the respawned worker resumes from it. Do not keep
  working into compaction.
- Dependency/conflict failure: compare manifests and target state against the
  supervisor's named branch. Checkpoint before rebasing. Resolve owned conflicts;
  escalate unowned ones with affected files and hashes.
- Failure outside your diff: compare the same check on the base when permitted;
  report exact output and which side failed. Rust checks/tests follow
  [discipline.md](discipline.md); an older runtime denial requires supervisor proof.
- Missing submodule: inspect the main checkout's initialized copy before choosing
  the project-supported linking/init route. Do not silently change dependencies.
