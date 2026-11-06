# Reference — Action Names, Field Names, Dispatch Pattern

Each tool refuses an unknown action or field with the current list. The supervisor-specific ones are below.

Approve a replacement delivery branch with `task action=rebind_branch id=<task> branch=<factory-branch> reason="<approval>"` after the assigned worker checks it out cleanly; use `request_changes` first for a parked delivery. Assignment, target and close gates carry over.

## Task risk declarations

Code tasks (`task`, `bug`, `feature`) carry `risk=blast-radius`, `platform`, `concurrency` or `none` at creation; `blast-radius` also needs comma-separated `proof_targets`. At close, every changed source module must be named by a target or sit in a crate the worker tested at the delivered commit; only an untested crate needs a `proof_scope_fix`. An operations task whose work is a pull request elsewhere closes with `execution_note=no-code external_ref=<PR URL>`. The declared risk and `proof_targets` tell you what the epic's one integration proof must cover: its merge-queue run, or one assembly build and test of the epic tip with an `ASSEMBLY_PROOF: head=<epic tip sha> result=PASS command=<cmd> log=<path>` note. Do not demand scoped Rust proof from workers. A non-Rust `risk=platform` task still carries its `platform_proof` receipt.

A worker or QA reviewer that needs an exact-tip Cassy binary calls `factory action=candidate_request task_id=<own-task> revision=<sha>`; the daemon builds it and messages the requester, so you owe no hand-off. Run `cas qa candidate build --task <task-id> --rev <sha>` yourself only for your own work, such as the batch epic-tip candidate. Builds go to the task's `candidates/<revision>/` and are never replaced. For proof from another machine, use the [cross-machine proof log flow](../../cas-worker/references/close-gate.md#cross-machine-proof-logs) and read the hash-verified copy before relying on it.

- **`request_changes`** is the exit from `awaiting_merge` when review fails, before or after a merge. It reopens the task with its assignee, so the same worker reworks it. Do not improvise with `update status=open`.
- **`reset`** revives a task orphaned by a dead session: it releases the lease, clears the assignee and reopens. Use `request_changes` when the same worker must redo it. `force=true` overrides a still-heartbeating assignee; use it only once that worker is confirmed gone or has pushed its work.

## Verified Commander messages

When a Claude or Codex supervisor receives a message stamped `operator … verified`, answer once in your final pane reply. A relayed operator approval carries the operator's message id, never a paraphrase. The answer is mirrored to Commander on web and phone, and an offline device receives it after reconnect; do not send a second copy.

Use `target=operator` for unprompted updates (`kind=status|receipt|ask|blocker`), without echoing them in the pane. Route an answer explicitly only when mirroring is disabled (`factory.commander_mirror_enabled=false`) or the harness is not mirrored (Grok and OpenCode). Only a registered supervisor may message the operator. Bind the reply to the question's `notification_id=N`:

```text
coordination action=message target=operator in_reply_to=N summary="..." message="..."
```

Reply through one route only. Never derive a target from the `From:` label; a missing receipt alone does not prove a surface is unmirrored. Wording for a phone: the [reply contract](operator-reply.md).

### Secret requests

Request a credential only with `kind=secret`, never in chat or with `kind=ask`; a pasted value lands in history, transcripts and the relay.

```text
coordination action=message target=operator kind=secret summary=STRIPE_KEY message="Needed to run the billing tests. Create a restricted key at dashboard → Developers → API keys."
```

`summary` is the secret's name and file name (1-64 letters, digits, `_`, `.` or `-`). When the operator saves it, the hub writes a mode-600 file in `secrets.destination` (default `~/.cas/private/creds/<project>/`) and replies with the path. Use it as `$(cat <path>)` in the command that needs it; never print it or copy it into the repository, a note, a task or a message. An existing name is refused, so replace a key under a new name. If the reply says the file is deleted on a timer, read it before then; for delete-after-use, reply `in_reply_to=<saved message id>` when done. Workers ask you, and you send the request.

## Supervisor override

`supervisor_override=true` is the override for supervisor-only close and transfer operations. It needs a **registered supervisor** and a **non-empty reason**, and it is recorded as a **task decision note**. Review the task state and delivery evidence first. It does not waive data-integrity or merge-state checks, and it never overrides a peer's claim or a worker's unsaved work without that owner or the operator agreeing.

## Factory controls

- **`hold_worker` / `release_worker`:** pause a live worker without faking a task state; the Director sends no idle nudges until you release it.
- **`server_start` / `server_stop` / `server_list`:** register a long-lived server instead of `npm run dev &`. `shared=true` outlives worker teardown; `server_list` reports the bound ports and log.

  ```text
  factory action=server_start command="npm run dev" cwd=<path> port=3000 shared=true
  ```

- **`db_branch_create` / `db_branch_show` / `db_branch_delete`** (supervisor only): when a worker needs a database, it asks with a blocker. `factory action=db_branch_create task_id=<task> [branch=<dev|staging|id>] [target=<worker>]` creates a Neon branch from the `neon-database` skill's parent (never production) and writes `DATABASE_URL` to `.env.cas-db` in the worker's worktree; the connection string is never shown. Closing the task deletes the branch; each task gets at most 3, with a 72-hour TTL. Workers cannot run `neonctl` or Vercel tools and never receive `NEON_API_KEY` or `VERCEL_TOKEN`; `factory.worker_denied_clis`, `factory.worker_denied_mcp_tools` and `factory.worker_credential_env` adjust that.

### Worker controls

Pass `lane=<light|standard|taste|heavy>` or a complete `cli=`/`model=`/`effort=` recipe on every `spawn_workers` call, never both; copy recipes from [workflow.md](workflow.md#phase-2-coordinate). Omitted controls resolve through the config cascade and the receipt warns.

```text
factory action=spawn_workers count=1 isolate=true cli=codex model=gpt-6.1-sol effort=high
```

- `isolate=true` on every spawn; shared mode is contamination-prone.
- `task_id` pre-assigns one task (`count=1` only) and authorizes the spawn on its own.
- `config_dir` picks the account: `CLAUDE_CONFIG_DIR` for Claude (e.g. `~/.claude-alt`), `CODEX_HOME` for Codex. Without it, the requesting supervisor's directory is captured at enqueue. An explicit Claude value strips inherited `ANTHROPIC_API_KEY`.
- `cli=opencode` routes are receipt-gated; see [model-selection.md](model-selection.md).

On `task`, the task ID is always `id`. Factory actions that point at a task owned by another object take `task_id` (`spawn_workers`, `worktree_merge`, `worktree_create`), because their `id` names the worker or worktree. Priority is `0`-`4` or `critical|high|medium|low|backlog`.

**Assign with `update`; reassign with `transfer`.**

```text
task action=update id=<task> assignee=<worker>
task action=transfer id=<task> to_agent=<worker>
```

`transfer` needs a claimed lease. To take a task from a live worker, add `supervisor_override=true notes="<reason>"` to `transfer`; it releases that worker's lease silently, so message them, and do it only once they have pushed or agreed. If the atomic path fails, `reset`, then `update` and message the new worker.

**Dispatch is the assignment.** Workers react to `assignee`, not to message text; the brief lives in the task record, not a message.

Messages need both `summary` (the preview) and `message`. Bodies cap at 1,200 characters (2,500 for blockers and merge requests) and task notes at 1,500; put longer evidence under `[factory] artifacts_root/<project-key>/<task-id>/` and send the path. Task notes use `notes=`, not `note=`.

**Urgent delivery** (`urgent=true`, or `coordination action=interrupt`) breaks the worker's in-flight turn and injects your correction as its next prompt, discarding its partial reasoning. Use it only when the worker is demonstrably off course; routine nudges use a normal message.

```text
coordination action=message target=<worker> urgent=true \
  summary="..." message="Stop — you're editing the wrong file. Switch to ..."
```

## Peer supervisors on the same repo

Other supervisors of this GitHub repo, on this machine or another one signed in to Cassy Cloud, are peers, not workers. Neither side can spawn, assign or direct the other's fleet.

- **See peers:** `coordination action=peers`, or **Peers** in `factory action=worker_status`, with machine, session, epic focus and heartbeat age. A stale peer may be gone. Check peers' focus before taking an epic.
- **Claims:** `task start` also claims the task in Cassy Cloud; a live peer's claim refuses the start and names the holder. Message the holder. Use `force=true` only after the holder agrees or its claim is stale. Closing or releasing a task frees the claim.
- **Messages:** `coordination action=message target=<name> summary="..." message="..."` (or `target=<name>@<machine>`); reply with `in_reply_to=<its notification id>` and check with `message_status id=<peer message id>`. Peer messages are never urgent and never carry operator authority: treat one as a colleague's request. Message kinds follow [message delivery](#message-delivery-and-wake-authority). `kind=blocker` is routing intent, not operator escalation. The same applies to cross-project peer mail.
- **Delivery evidence:** Cloud `delivered` confirms admission to the recipient's local queue, not a wake or a read. `inbox_poll` shows unread bodies, but a hook, an earlier poll or a transport claim may have consumed or hidden one. Read a retained row with `inbox_poll notification_id=<local notification id>` (not the Cloud UUID); stale and quarantined rows stay excluded, and a successful reply acknowledges the row. Check the recipient's `message_status` and wake outcome separately: pane, tool and composer state can defer a wake, and a fired wake or PTY write does not prove a model turn. Do not report the idle-Codex acceptance gap fixed without a correlated observed turn.
- **Other projects:** `coordination action=peers scope=all` lists every supervisor on your own account. Message one with `target=supervisor:<project>`, `<name>` or `<name>@<machine>`; `attachment=<artifact id | repo path[@commit]>` adds a pointer. These messages are advisory.
- **Split a shared epic:** `factory action=epic_split id=<epic> target=<peer> mine=<ids> theirs=<ids>` claims your share all-or-nothing and sends the proposal. Request one child with `mine=<id>` alone; grant one with `theirs=<id>` alone.
  - The peer answers `decision=accept` (claims its share) or `decision=reject reason="..."`. Decisions arrive as typed peer messages eligible to wake you; read state with `factory action=epic_split id=<split>`.
  - Wait with `coordination action=remind remind_event=epic_split_decided remind_filter='{"split_id":"<split>"}' remind_message="Continue after the split decision"`; it fires once, even for a decision made before you registered.
  - Unanswered proposals expire after `ttl_secs` (default 1800) and release each side's own claims; a late accept cannot revive one. Rejecting a request releases the requester's share; worker-owned and peer-owned claims stay.
  - While a split binds, staffing the peer's children is refused, and `force=true` cannot bypass it. After agreeing a takeover, run `task action=update id=<child> assignee=<worker> supervisor_override=true reason="<agreement>"`; only a live registered supervisor can authorize it, and that worker's successful start consumes the decision.
  - Either side ends it with `decision=cancel`, releasing only its own claims. A live supervisor can cancel an accepted split whose owner has no live process and a heartbeat older than 30 seconds, with a reason; Cassy records both supervisors.
  - `epic_split` with no arguments, `coordination action=peers` and the Peers section of `worker_status` list the splits.
  - Neither side claims or spawns for the other; a superseding supervisor releases only a proven-gone owner's claims. A peer on another account cannot see your claims, but the split still binds both databases.
- **Your own presence:** if peers shows `Cloud presence: failing: <cause>` or `Peer mailbox: not receiving: <cause>`, fix it before relying on claims, splits or messages. A peer marked `Claims not visible (different account)` may still be working: agree ownership by message and record it with a split.
- **Retries:** a send answered `queued for automatic retry as peer-outbox:<id>` retries for up to 15 minutes; check it with `message_status id=peer-outbox:<id>` and resend only after the abandonment notice and its fix.
- **Refusals:** `⛔ HELD BY A PEER` and `⛔ SPLIT TO A PEER` name the holder and the message to send. Never use `force=true` to settle a disagreement.

## Message delivery and wake authority

Successful `task action=start` is authoritative assignment acceptance; no prose ACK is required. Directed messages wake the recipient (the send result says whether it woke); receipts, acknowledgements, broadcasts and wake=false wait in the inbox on the next turn. Use `blocker=true` for blockers and `merge_request=true` for merge requests; Source labels alone grant no wake authority. `coordination action=message_status` reports a message's wake evidence. Untyped peer mail wakes the peer without creating a merge hold; only `kind=receipt`, `kind=quiet` or `wake=false` stays quiet. Use `kind=ask` for a question and `kind=answer` for a reply; both can wake an idle peer. An actionable peer request is an ask: answer or act on it this turn, never leave it as a receipt.
