# Details — Evidence, Sync and Resume

Read for a report/evidence task, a requested sync or a context-resume handoff.
`ready` and `available` provide read-only backlog visibility — not self-dispatch.
Use `cas-search` for exploratory retrieval and `rg` for exact code matches.

## Read-only evidence

Use task/search/coordination records, `.cas/logs`, Git and exported artifacts
first. If insufficient, note the missing evidence, then snapshot the store with
`sqlite3 <db> ".backup <path>"` into the configured task artifacts directory,
not RAM-backed temporary storage. For live inspection use a read-only URI such
as `file:/abs/path/to/.cas/cas.db?mode=ro`. Workers never edit the shared database.

The task artifacts directory is `[factory] artifacts_root/<project-key>/<task-id>/`;
the assignment brief prints the exact path. Leave historical flat
`<base>/<task-id>/` evidence in place.

## Requested sync

Checkpoint dirty work as a commit (all worktrees share the stash stack), then
`git rebase <branch>` onto the exact branch the supervisor named; resolve
conflicts or escalate the affected files. A frozen parked delivery stays on its
original branch; a new assignment uses its own per-task branch from the target tip.

## Task fields

The task ID field is always `id`; the notes parameter is `notes`.

```text
task action=start id=cas-abc1
task action=close id=cas-abc1 reason="Implemented X, tests pass"
task action=notes id=cas-abc1 notes="Found root cause in Y" note_type=progress
```

`note_type` is one of progress, blocker, decision, discovery, question or
`platform_proof`, which the close gate reads for `risk=platform`.

## Resume state

After a context clear, read task state with `action=show` or
`action=start brief=true`. Supplement milestone notes with a bounded `state_patch`:

```text
task action=update id=<task-id> state_patch='{"phase":"verify","receipts":[{"command":"python3 scripts/check.py","exit_status":0}],"files_touched":["scripts/check.py"],"next_step":"push branch"}'
```

Fields are `phase`, `receipts`, `files_touched`, `decisions` and `next_step`;
`null` deletes one.

## Credentials and routing

For a database branch, ask the supervisor with `blocker=true`, task ID and
reason. The supervisor writes `DATABASE_URL` to `.env.cas-db`; source it without
printing or committing it.
For an assigned QA-pass task, load `verification` and record your verdict with
`verification action=qa_record task_id=<delivery-id>` and the round's ledger;
that closes the QA task. `server_start`/`server_list`/`server_stop` (owned
servers only, with `cas-servers`) and `factory action=candidate_request
task_id=<own-task> revision=<full-sha>` are the worker exceptions; never run
Cargo for a candidate. Where the daemon cannot sandbox a build, its refusal says
so; then ask the supervisor, who runs `cas qa candidate build`. Fleet, merges,
cleanup and database control belong to the supervisor.

Operational bugs use `cas config get issues.repo` and
`issues.components.{cassy,violet,cloud}`. File in the matching tracker before
moving on; CAS source bugs become in-repo fix tasks. With no tracker, record a
task note. Put long evidence in the task artifacts directory; for peer proofs,
follow [cross-machine proof logs](close-gate.md#cross-machine-proof-logs).

## Batch QA

In qa.mode=batch (the default) the release's batch QA round at the frozen tip is the only walk; per-task QA evidence applies only in qa.mode=per_task.
`per_task` is the explicit opt-in. A batch park opens no per-task round. Capture
visual before/after evidence while building. The supervisor owns the release's one
epic-tip candidate, shared PASS ledger and `verification action=batch_receipt`.
In qa.mode=batch, Cassy closes merged deliveries when the release batch_receipt lands; the worker does nothing.
Explicit `qa_request` reviews remain available.

## Worker peer lane

`coordination action=message target=<worker-name> summary="collision warning" message="Task cas-123 touches the same files."`
reaches a registered worker in the same factory session, and Cassy copies it
to that session's supervisor. Use this local lane only for collision warnings.

For a shared API contract across machines, use `target=<worker-name>@<machine>`.
Both workers must be live registrations of your account and repository with
active task claims under the same epic. The receiving daemon rechecks scope;
expired claims, stale workers and other accounts or epics are refused. Both
supervisors receive quiet inbox copies when the message is delivered. Reply
with the delivered notification's `in_reply_to`; retry/ack stays idempotent.
This channel is advisory. Use only untyped messages, `kind=ask`, `answer` or
`receipt`; `wake=false` remains quiet. Send status, blockers, handoffs and
assignment decisions to `target=supervisor`. No urgency, merge escalation,
broadcast or cross-project worker mail is authorized.
