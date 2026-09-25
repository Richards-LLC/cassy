# Recovery — Failure Modes and What to Do

## Close hit ⚠️ MERGE REQUIRED (merge-state guard)

The most common close rejection: your `factory/<name>` branch has commits not yet on the task's parent branch. This is a **data-state guard** — supervisor overrides do not apply.

1. **Read the guard text** — it names the parent branch and the unmerged commit count, and includes the correct remediation for your case.
2. **Before any escalation, drain the inbox: run `cas__coordination action=inbox_poll` repeatedly until it returns `No unread messages`.** A poll returns at most 10 rows by default, so one call is not guaranteed to pull all unread supervisor messages. Polling marks messages seen for inbox polling without consuming daemon transport delivery, and the claim is at-most-once — if a poll response is lost those rows are not replayed, so also re-read any supervisor messages just delivered in your conversation. If a message says the branch was merged or requests more changes, follow it and do not send a stale merge request.
3. **`delivery_mode=local_merge`**: run `git rev-parse factory/<name>` to capture the current tip, then send the supervisor a merge request with that SHA. Do **not** push origin; the supervisor merges your local factory branch.
4. **Parent is `epic/<slug>` (`delivery_mode=push_branch`)**: run `git rev-parse factory/<name>` to capture the current tip, `git push origin factory/<name>`, then message the supervisor to merge your branch into the epic:
   ```
   cas__coordination action=message target=supervisor \
     summary="factory/<name> pushed, needs epic merge before close" \
     message="Fresh after polling unread inbox messages: <task-id> factory/<name> tip <sha>. Please re-check reachability, then merge into epic/<slug> if still needed so close can pass."
   ```
   Do **NOT** `gh pr create --base epic/...` — epic branches are supervisor-local; the ref doesn't exist on origin and the call always fails.
5. **Parent is `main`/`master`/`staging`**: push and complete the project's PR/merge flow, then retry close.
6. **Guard still counts unmerged commits after a confirmed merge** → squash-merge SHA drift. Clear it yourself first: re-close with `commit_receipt=<sha>` naming the commit that carries this task's work. Only if the receipt is rejected, send the supervisor the exact guard text *and* the rejection reason. Do not retry-loop against the guard. `completion_receipt` is accepted only after your current tip is merged. Both receipts are specified in [close-gate.md](close-gate.md) "Delivery receipts".
7. **Never route around it** with `action=update status=closed` plus a hand-written verification record; see [close-gate.md](close-gate.md) "Never bypass the close path".

## Close requires task-scoped verification

1. **Forward ONCE** to supervisor via `cas__coordination action=message` — include task ID, brief summary of completion state, and exact error text. The close response names the affected task, dispatch owner, deadline, and recovery path; copy that guidance directly.
2. **Do not re-report.** The supervisor will verify and close asynchronously. Re-sending the same message does not speed this up.
3. **Continue unrelated work.** Verification gates only the named task's transition to closed; unrelated MCP and other-task work remain available. When idle, re-check `cas__task action=show id=<your-task-id>`. If `Status: Closed`, trust the DB over messages.
4. **If still InProgress after 5 minutes of idle**, send ONE follow-up to the supervisor with note_type=blocker. Then continue to re-poll DB only.
5. **Never spam idle notifications as a substitute for work.** If you are idle waiting on verification, stay silent until (a) the DB shows closed and you proceed to the next task, or (b) 5 minutes have elapsed and you send the one follow-up.

## ALL tools blocked (stale-binary universal jail)

If **every** MCP tool call fails with a jail/blocked error (not just the named task's close/update-to-closed), the running Cassy binary predates task-scoped verification enforcement.

1. **Do NOT attempt workarounds** — no sqlite edits, no env var hacks, no retries.
2. **Report to supervisor immediately** via `cas__coordination action=message` with the exact error message and your agent name.
3. **Supervisor will rebuild Cassy and respawn you.** This is not something you can fix from inside your session.

## Context Exhaustion

If your output degrades to garbled multi-language text, or you find yourself repeating the same fix in a loop, this is context exhaustion (attention collapse from a long session). You cannot self-recover from this state.

Message supervisor immediately: "Context exhausted, need respawn." Do not attempt to continue working.

Prevention: below 20 % headroom, checkpoint (commit, push or park, handoff note, respawn request) before you reach this state.

## Worktree Issues (Isolated Mode)

**Submodule not initialized**: Worktrees don't include submodules. Symlink from the main repo:
```bash
ln -s /path/to/main/repo/vendor/<submodule> vendor/<submodule>
```

**Failures in code you didn't touch**: Triage before reporting to supervisor. Do not build or test Rust to triage; the supervisor's assembly build covers Rust.

1. **Merge conflict from another worker?** Checkpoint uncommitted work as a commit, then rebase onto the **local** branch the supervisor named at assignment (`main`, `master`, or `epic/<slug>`); see [details.md](details.md) "Syncing". Do **not** rebase onto `origin/<branch>`: the supervisor merges into the local branch, so `origin/main` is stale and `origin/epic/...` does not exist. If conflicts appear in files you own, resolve them; if in files you don't own, report to supervisor.

2. **Missing dependency or new module?** Check if another worker added dependencies, diffing against that same local branch:
   ```bash
   git diff <branch> -- Cargo.toml Cargo.lock package.json pnpm-lock.yaml
   ```
   If new crates/packages were added, rebase onto it.

3. **Non-Rust failure: reproducible on the base branch?** Commit your work first, then `git switch --detach <branch>`, run the same non-Rust command, and `git switch -` back.
   - If it fails there too → report to supervisor as **pre-existing** (not your blocker).
   - If it passes there → report a **cross-worker conflict** with both commit hashes.

Only report to supervisor after completing at least steps 1–2. Include the error output and which step identified the cause.

## MCP Connectivity Failure

If `cas__*` tools stop responding or return connection errors:

1. **Check the symlink**: Worktrees get MCP config via symlink, not a copy.
   ```bash
   ls -la .mcp.json  # Should be a symlink to main repo's .mcp.json
   ```
   If the symlink is broken or missing, the MCP server can't start.

2. **Check the Cassy server process**: The `cas serve` process may have crashed.
   ```bash
   ps aux | grep 'cas serve'
   ```

3. **Do NOT attempt sqlite surgery.** Direct database edits from a worker session risk corrupting shared state.

4. **Report to supervisor** via `cas__coordination action=message` with the error and diagnostic output. Supervisor will fix the MCP connection or respawn you.

## ToolSearch resolved the tool but you still can't call it

Symptom: `ToolSearch(query="select:cas__task")` returns a match for `cas__task` (or any `cas__*` tool), but you're unsure how to proceed and are tempted to run ToolSearch again "to make it callable."

**Do not re-run ToolSearch for a tool it already resolved — that will not make it more callable.** A successful ToolSearch match means the tool is now loadable; the very next action is a *separate* tool call literally named `cas__task` (or whichever tool matched), passing your real arguments (e.g. `action=mine`). If that direct call then fails or the tool name is rejected as unknown, treat it as **Zero Cassy Tools Available** below and report to the supervisor — don't loop on ToolSearch, and don't fall back to `cas task ...` as a shell command (no such CLI subcommand exists).

## Zero Cassy Tools Available

(no `cas__*` tools surfaced at all — not one call errors, they simply do not exist in your tool set)

This is different from connectivity failure above. Here the MCP handshake completed against *something*, but `cas serve` either crashed during startup or silently degraded before registering its tools. Symptom: `ToolSearch select:cas__task` returns `"No matching deferred tools found"` even though other MCP servers (e.g. Gmail, Calendar) are present.

**Do not** fall back to running `cas task` as a shell subcommand — it does not exist. **Do not** run `cas init` from inside the worktree (creates a duplicate `.cas/`). **Do not** kill/restart `cas serve` yourself.

Report to supervisor immediately with:
```
cas__coordination action=message target=supervisor \
  summary="zero cas tools available" \
  message="<your-name>: no cas__* tools in tool set. Need respawn."
```

If even `cas__coordination` is missing (so you cannot send that message), you are fully detached. Output a short plain-text report and stop — the supervisor polls your session and will detect the stall. Do not spin attempting workarounds.

## Known-fixed Cassy bug reappears

If a bug that was supposedly fixed in the source code still manifests, the running Cassy binary may be outdated (not rebuilt after the fix). Report to supervisor — don't file a duplicate bug or attempt your own fix.

## Supervisor goes silent

If the supervisor hasn't responded after 5 minutes on any blocking question:
1. Re-read task state with `action=show` — supervisor may have acted without messaging back.
2. Send ONE follow-up via `cas__coordination action=message`.
3. If still no response after another 5 minutes, focus on any non-blocked work or pause. Do not spam.

## Task Reassigned While Working

If the supervisor reassigns your current task to another worker:

1. **Commit WIP immediately** (`git add <paths> && git commit -m "WIP: <task-id> handoff"`) — do not lose work in progress, and do not use `git stash`: the stash stack is shared by every worktree.
2. **Post progress notes** summarizing what's done and what's left:
   ```
   cas__task action=notes id=<task-id> notes="WIP: <what's done>, remaining: <what's left>" note_type=progress
   ```
3. **Message supervisor** with the commit SHA of your WIP so the new assignee can pick it up.
4. **Stop work on that task immediately** — do not finish "just one more thing." Move to your next assigned task or check `cas__task action=mine`.

## Outbox replay

Your outbox may replay stale messages after task state changes (delivery-layer artifact). Before re-sending a blocker or completion notification, re-check task state with `cas__task action=show` — the issue may already be resolved.
