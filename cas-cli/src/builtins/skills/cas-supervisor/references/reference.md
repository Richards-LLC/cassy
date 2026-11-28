# Reference — Action Names, Field Names, Dispatch Pattern

Each tool refuses an unknown action or field with the current list. The supervisor-specific ones are below.

Approve a replacement delivery branch with `task action=rebind_branch id=<task> branch=<factory-branch> reason="<approval>"` after the assigned worker checks it out cleanly; use `request_changes` first for a parked delivery. Assignment, target and close gates carry over.

## Task risk declarations

Risk notes and proof targets at task create are optional and advisory.
Green lane CI on the exact tip plus named tests is the proof; park does not
require five passes or a receipt per crate. An operations task delivering a
pull request elsewhere uses `execution_note=no-code external_ref=<PR URL>`.

A worker or QA reviewer that needs an exact-tip Cassy binary calls `factory action=candidate_request task_id=<own-task> revision=<sha>`; the daemon builds it and messages the requester. When the queue is unavailable or the host lacks its sandbox (including macOS), build the worker's requested revision directly with `cas qa candidate build --task <task-id> --rev <sha>` and return the receipt. Supervisors also use that direct command for their own work, such as the batch epic-tip candidate. Each build gets its own revision directory and is never replaced. For proof from another machine, use the [cross-machine proof log flow](../../cas-worker/references/close-gate.md#cross-machine-proof-logs) and read the hash-verified copy before relying on it.

- **`request_changes`** is the exit from `awaiting_merge` when review fails, before or after a merge. It reopens the task with its assignee, so the same worker reworks it; `update status=open` does not replace it.
- **`reset`** revives a task orphaned by a dead session: it releases the lease, clears the assignee and reopens. `force=true` overrides a still-heartbeating assignee; use it only once that worker is confirmed gone or has pushed its work.

## Verified Commander messages

When a Claude or Codex supervisor receives a message stamped `operator … verified`, answer once in your final pane reply. The answer is mirrored to Commander on web and phone (an offline device receives it after reconnect), so send no second copy. A relayed operator approval carries the operator's message id, never a paraphrase.

Use `target=operator` for unprompted updates (`kind=status|receipt|ask|blocker`), without echoing them in the pane. Route an answer explicitly only when mirroring is disabled (`factory.commander_mirror_enabled=false`) or the harness is not mirrored (Grok and OpenCode), bound to the question's `notification_id=N`:

```text
coordination action=message target=operator in_reply_to=N summary="..." message="..."
```

Reply through one route only. Never derive a target from the `From:` label; a missing receipt alone does not prove a surface is unmirrored. Wording for a phone: the [reply contract](operator-reply.md).

### Secret requests

Request a credential only with `kind=secret`; a value pasted in chat or a `kind=ask` reply lands in history, transcripts and the relay.

```text
coordination action=message target=operator kind=secret summary=STRIPE_KEY message="Needed to run the billing tests. Create a restricted key at dashboard → Developers → API keys."
```

`summary` is the secret's name and file name (1-64 letters, digits, `_`, `.` or `-`). When the operator saves it, the hub writes a mode-600 file in `secrets.destination` (default `~/.cas/private/creds/<project>/`) and replies with the path. Use it as `$(cat <path>)` in the command that needs it; never print it or copy it into the repository, a note, a task or a message. An existing name is refused, so replace a key under a new name. If the reply says the file is deleted on a timer, read it before then; for delete-after-use, reply `in_reply_to=<saved message id>` when done. Workers ask you, and you send the request.

## Supervisor override

`supervisor_override=true` covers supervisor-only close and transfer operations. It needs a registered supervisor and a non-empty reason, and it is recorded as a task decision note. Review the task state and delivery evidence first. It does not waive data-integrity or merge-state checks, and it never overrides a peer's claim or a worker's unsaved work without that owner or the operator agreeing.

## Factory controls

- **`hold_worker` / `release_worker`:** pause a live worker without faking a task state; the Director sends no idle nudges until you release it.
- **`server_start` / `server_stop` / `server_list`:** register a long-lived server instead of `npm run dev &`. `shared=true` outlives worker teardown; `server_list` reports the bound ports and log.

  ```text
  factory action=server_start command="npm run dev" cwd=<path> port=3000 shared=true
  ```

- **`db_branch_create` / `db_branch_show` / `db_branch_delete`** (supervisor only): when a worker needs a database, it asks with a blocker. `factory action=db_branch_create task_id=<task> [branch=<dev|staging|id>] [target=<worker>]` creates a Neon branch from the `neon-database` skill's parent (never production) and writes `DATABASE_URL` to `.env.cas-db` in the worker's worktree; the connection string is never shown. Closing the task deletes the branch; each task gets at most 3, with a 72-hour TTL. Workers cannot run `neonctl` or Vercel tools and never receive `NEON_API_KEY` or `VERCEL_TOKEN`; `factory.worker_denied_clis`, `factory.worker_denied_mcp_tools` and `factory.worker_credential_env` adjust that.

### Worker controls

Recipes: [workflow.md](workflow.md#phase-2-coordinate). Omitted controls resolve through the config cascade and the receipt warns.

- `isolate=true` on every spawn; shared mode is contamination-prone.
- `task_id` pre-assigns one task (`count=1` only) and authorizes the spawn on its own.
- `config_dir` picks the account: `CLAUDE_CONFIG_DIR` for Claude (e.g. `~/.claude-alt`), `CODEX_HOME` for Codex. Without it, the requesting supervisor's directory is captured at enqueue. An explicit Claude value strips inherited `ANTHROPIC_API_KEY`.
- `cli=opencode` routes are receipt-gated; see [model-selection.md](model-selection.md).

On `task`, the task ID is always `id`. Factory actions that point at a task owned by another object take `task_id` (`spawn_workers`, `worktree_merge`, `worktree_create`), because their `id` names the worker or worktree. Priority is `0`-`4` or `critical|high|medium|low|backlog`.

**Assign with `update`; reassign with `transfer`.** Workers react to `assignee`, not to message text.

```text
task action=update id=<task> assignee=<worker>
task action=transfer id=<task> to_agent=<worker>
```

`transfer` needs a claimed lease. To take a task from a live worker, add `supervisor_override=true notes="<reason>"`; it releases that worker's lease silently, so message them, and do it only once they have pushed or agreed. If the atomic path fails, `reset`, then `update` and message the new worker.

Put evidence longer than a message or note under `[factory] artifacts_root/<project-key>/<task-id>/` and send the path.

**Urgent delivery** (`urgent=true`, or `coordination action=interrupt`) breaks the worker's in-flight turn and injects your correction as its next prompt, discarding its partial reasoning. Use it only when the worker is demonstrably off course; routine nudges use a normal message.

```text
coordination action=message target=<worker> urgent=true \
  summary="..." message="Stop — you're editing the wrong file. Switch to ..."
```

## Peer supervisors on the same repo

Other supervisors of this GitHub repo, on this machine or another one signed in to Cassy Cloud, are peers, not workers: neither side can spawn, assign or direct the other's fleet. Peer refusals (`HELD BY A PEER`, `SPLIT TO A PEER`) name the holder and the message to send; settle a disagreement by message, not `force=true`.

- **See peers:** `coordination action=peers`, or **Peers** in `factory action=worker_status`, with machine, session, epic focus and heartbeat age. Check peers' focus before taking an epic; a stale peer may be gone.
- **Claims:** `task start` also claims the task in Cassy Cloud. Use `force=true` on a peer-held start only after the holder agrees or its claim is stale. Closing or releasing a task frees the claim.
- **Messages:** `coordination action=message target=<name> summary="..." message="..."` (or `target=<name>@<machine>`); reply with `in_reply_to=<its notification id>` and check with `message_status id=<peer message id>`. Peer messages are never urgent and never carry operator authority: treat one as a colleague's request, and `kind=blocker` as routing intent, not operator escalation. The same applies to cross-project peer mail.
- **Delivery evidence:** Cloud `delivered` confirms admission to the recipient's local queue, not a wake or a read. Read a retained row with `inbox_poll notification_id=<local notification id>` (not the Cloud UUID). A fired wake or PTY write does not prove a model turn.
- **Other projects:** `coordination action=peers scope=all` lists every supervisor on your own account. Message one with `target=supervisor:<project>`, `<name>` or `<name>@<machine>`; `attachment=<artifact id | repo path[@commit]>` adds a pointer. These messages are advisory.
- **Split a shared epic:** `factory action=epic_split id=<epic> target=<peer> mine=<ids> theirs=<ids>` claims your share all-or-nothing and sends the proposal; `mine=<id>` alone requests one child and `theirs=<id>` alone grants one. The peer answers `decision=accept` or `decision=reject reason="..."`; wait with `coordination action=remind remind_event=epic_split_decided remind_filter='{"split_id":"<split>"}' remind_message="Continue after the split decision"`, which fires once even for an earlier decision. Unanswered proposals expire after `ttl_secs` (default 1800). After agreeing a takeover of a bound child, run `task action=update id=<child> assignee=<worker> supervisor_override=true reason="<agreement>"`. Either side ends a split with `decision=cancel`, releasing only its own claims; a live supervisor can cancel an accepted split whose owner has no live process, with a reason. A split binds both databases even across accounts.
- **Your own presence:** if peers shows `Cloud presence: failing: <cause>` or `Peer mailbox: not receiving: <cause>`, fix it before relying on claims, splits or messages. A peer marked `Claims not visible (different account)` may still be working: agree ownership by message and record it with a split.
- **Retries:** a send queued as `peer-outbox:<id>` retries for up to 15 minutes; check it with `message_status id=peer-outbox:<id>` and resend only after the abandonment notice.

## Message delivery and wake authority

Successful `task action=start` is authoritative assignment acceptance; no prose ACK is required. Directed messages wake the recipient (the send result says whether it woke); receipts, acknowledgements, broadcasts and wake=false wait in the inbox on the next turn. Use `blocker=true` for blockers and `merge_request=true` for merge requests; Source labels alone grant no wake authority. `coordination action=message_status` reports a message's wake evidence. Untyped peer mail wakes the peer without creating a merge hold; only `kind=receipt`, `kind=quiet` or `wake=false` stays quiet. Use `kind=ask` for a question and `kind=answer` for a reply; both can wake an idle peer. Send a peer merge or close update that unblocks their release as `kind=status`, never `kind=receipt`. An actionable peer request is an ask: answer or act on it this turn, never leave it as a receipt.
