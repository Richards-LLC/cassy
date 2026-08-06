# Recovery — Failure Modes and What to Do

## Close hit ⚠️ MERGE REQUIRED (merge-state guard)

The most common close rejection: your `factory/<name>` branch has commits not yet on the task's parent branch. This is a **data-state guard** — nobody can bypass it; `bypass_code_review=true` does not apply.

1. **Read the guard text** — it names the parent branch and the unmerged commit count, and includes the correct remediation for your case.
2. **Before any escalation, run `mcp__cs__coordination action=inbox_poll`.** This pulls unread supervisor messages and marks them seen for inbox polling without consuming daemon transport delivery. If a message says the branch was merged or requests more changes, follow it and do not send a stale merge request.
3. **Parent is `epic/<slug>`**: run `git rev-parse factory/<name>` to capture the current tip, `git push origin factory/<name>`, then message the supervisor to merge your branch into the epic:
   ```
   mcp__cs__coordination action=message target=supervisor \
     summary="factory/<name> pushed, needs epic merge before close" \
     message="Fresh after polling unread inbox messages: <task-id> factory/<name> tip <sha>. Please re-check reachability, then merge into epic/<slug> if still needed so close can pass."
   ```
   Do **NOT** `gh pr create --base epic/...` — epic branches are supervisor-local; the ref doesn't exist on origin and the call always fails.
4. **Parent is `main`/`master`/`staging`**: push and complete the project's PR/merge flow, then retry close.
5. **Guard still counts unmerged commits after a confirmed merge** → squash-merge SHA drift makes already-merged commits look missing. Send the supervisor the exact guard text (they reset the stale branch ref). Do not retry-loop against the guard.
6. **Never route around it** with `action=update status=closed` plus a hand-written `verification action=add` — that forges the verification record and the audit trail. Rejection loops are a supervisor conversation, not a workaround opportunity.

## Close requires task-scoped verification

1. **Forward ONCE** to supervisor via `mcp__cs__coordination action=message` — include task ID, brief summary of completion state, and exact error text. The close response names the affected task, dispatch owner, deadline, and recovery path; copy that guidance directly.
2. **Do not re-report.** The supervisor will verify and close asynchronously. Re-sending the same message does not speed this up.
3. **Continue unrelated work.** Verification gates only the named task's transition to closed; unrelated MCP and other-task work remain available. When idle, re-check `mcp__cs__task action=show id=<your-task-id>`. If `Status: Closed`, trust the DB over messages.
4. **If still InProgress after 5 minutes of idle**, send ONE follow-up to the supervisor with note_type=blocker. Then continue to re-poll DB only.
5. **Never spam idle notifications as a substitute for work.** If you are idle waiting on verification, stay silent until (a) the DB shows closed and you proceed to the next task, or (b) 5 minutes have elapsed and you send the one follow-up.

## ALL tools blocked (stale-binary universal jail)

If **every** MCP tool call fails with a jail/blocked error (not just the named task's close/update-to-closed), the running CAS binary predates task-scoped verification enforcement.

1. **Do NOT attempt workarounds** — no sqlite edits, no env var hacks, no retries.
2. **Report to supervisor immediately** via `mcp__cs__coordination action=message` with the exact error message and your agent name.
3. **Supervisor will rebuild CAS and respawn you.** This is not something you can fix from inside your session.

## Context Exhaustion

If your output degrades to garbled multi-language text, or you find yourself repeating the same fix in a loop, this is context exhaustion (attention collapse from a long session). You cannot self-recover from this state.

Message supervisor immediately: "Context exhausted, need respawn." Do not attempt to continue working.

## Worktree Issues (Isolated Mode)

**Submodule not initialized**: Worktrees don't include submodules. Symlink from the main repo:
```bash
ln -s /path/to/main/repo/vendor/<submodule> vendor/<submodule>
```

**Build errors in code you didn't touch**: Triage before reporting to supervisor.

1. **Merge conflict from another worker?** Pull latest from main and rebase:
   ```bash
   git fetch origin main && git rebase origin/main
   ```
   If conflicts appear in files you own, resolve them. If in files you don't own, report to supervisor.

2. **Missing dependency or new module?** Check if another worker added dependencies:
   ```bash
   git diff origin/main -- Cargo.toml Cargo.lock package.json pnpm-lock.yaml
   ```
   If new crates/packages were added, pull main and rebuild.

3. **Environment issue?** Verify tool versions and env vars match what the project expects:
   ```bash
   rustc --version && cargo --version  # Check Rust toolchain
   node --version                       # Check Node if applicable
   ```

4. **Reproducible on main?** Test whether the failure is pre-existing:
   ```bash
   git stash && git checkout origin/main && cargo build  # or npm run build
   ```
   - If it fails on main too → report to supervisor as **pre-existing** (not your blocker).
   - If it passes on main → the conflict is between your changes and another worker's recent commit. Report as **cross-worker conflict** with both commit hashes.

Only report to supervisor after completing at least steps 1–2. Include the error output and which step identified the cause.

## MCP Connectivity Failure

If `mcp__cs__*` tools stop responding or return connection errors:

1. **Check the symlink**: Worktrees get MCP config via symlink, not a copy.
   ```bash
   ls -la .mcp.json  # Should be a symlink to main repo's .mcp.json
   ```
   If the symlink is broken or missing, the MCP server can't start.

2. **Check the CAS server process**: The `cas serve` process may have crashed.
   ```bash
   ps aux | grep 'cas serve'
   ```

3. **Do NOT attempt sqlite surgery.** Direct database edits from a worker session risk corrupting shared state.

4. **Report to supervisor** via `mcp__cs__coordination action=message` with the error and diagnostic output. Supervisor will fix the MCP connection or respawn you.

## ToolSearch resolved the tool but you still can't call it

Symptom: `ToolSearch(query="select:mcp__cs__task")` returns a match for `mcp__cs__task` (or any `mcp__cs__*` tool), but you're unsure how to proceed and are tempted to run ToolSearch again "to make it callable."

**Do not re-run ToolSearch for a tool it already resolved — that will not make it more callable.** A successful ToolSearch match means the tool is now loadable; the very next action is a *separate* tool call literally named `mcp__cs__task` (or whichever tool matched), passing your real arguments (e.g. `action=mine`). If that direct call then fails or the tool name is rejected as unknown, treat it as **Zero CAS Tools Available** below and report to the supervisor — don't loop on ToolSearch, and don't fall back to `cas task ...` as a shell command (no such CLI subcommand exists).

## Zero CAS Tools Available

(no `mcp__cs__*` tools surfaced at all — not one call errors, they simply do not exist in your tool set)

This is different from connectivity failure above. Here the MCP handshake completed against *something*, but `cas serve` either crashed during startup or silently degraded before registering its tools. Symptom: `ToolSearch select:mcp__cs__task` returns `"No matching deferred tools found"` even though other MCP servers (e.g. Gmail, Calendar) are present.

**Do not** fall back to running `cas task` as a shell subcommand — it does not exist. **Do not** run `cas init` from inside the worktree (creates a duplicate `.cas/`). **Do not** kill/restart `cas serve` yourself.

Report to supervisor immediately with:
```
mcp__cs__coordination action=message target=supervisor \
  summary="zero cas tools available" \
  message="<your-name>: no mcp__cs__* tools in tool set. Need respawn."
```

If even `mcp__cs__coordination` is missing (so you cannot send that message), you are fully detached. Output a short plain-text report and stop — the supervisor polls your session and will detect the stall. Do not spin attempting workarounds.

## Known-fixed CAS bug reappears

If a bug that was supposedly fixed in the source code still manifests, the running CAS binary may be outdated (not rebuilt after the fix). Report to supervisor — don't file a duplicate bug or attempt your own fix.

## Supervisor goes silent

If the supervisor hasn't responded after 5 minutes on any blocking question:
1. Re-read task state with `action=show` — supervisor may have acted without messaging back.
2. Send ONE follow-up via `mcp__cs__coordination action=message`.
3. If still no response after another 5 minutes, focus on any non-blocked work or pause. Do not spam.

## Task Reassigned While Working

If the supervisor reassigns your current task to another worker:

1. **Commit or stash WIP immediately** — do not lose work in progress.
2. **Post progress notes** summarizing what's done and what's left:
   ```
   mcp__cs__task action=notes id=<task-id> notes="WIP: <what's done>, remaining: <what's left>" note_type=progress
   ```
3. **Message supervisor** with the commit SHA of your WIP so the new assignee can pick it up.
4. **Stop work on that task immediately** — do not finish "just one more thing." Move to your next assigned task or check `mcp__cs__task action=mine`.

## Outbox replay

Your outbox may replay stale messages after task state changes (delivery-layer artifact). Before re-sending a blocker or completion notification, re-check task state with `mcp__cs__task action=show` — the issue may already be resolved.

## A build that looks stuck: killed vs wedged

These are different failures with the same symptom (no output, no progress), and
telling them apart takes about ten seconds. **Do not wait it out** — a wedged
build never recovers on its own, and one was observed sitting for 57 minutes.

**First, read the build log, not the clock.** Cargo already reports a killed
child clearly:

```
error: could not compile `foo` (signal: 9, SIGKILL: kill)
```

If you see that, the build **failed** — it did not hang. Something killed the
compiler. Re-run it. If it recurs, find out who is sending the signal before
blaming the machine.

**If there is no such line and nothing is moving, inspect the process:**

```bash
ps -eo pid,etime,time,stat,wchan:20,comm | grep -E "rustc|cargo"
```

Read two columns together:

| `TIME` (CPU used) | Meaning |
|---|---|
| climbing | It is compiling. Slow ≠ stuck. Leave it alone. |
| ~0:00 with large `ETIME` | Wedged. It has been alive for minutes and burned no CPU. |

Confirm before concluding it is a resource problem:

```bash
grep oom_kill /proc/vmstat        # 0 => the kernel has killed nothing, ever
cat /proc/pressure/memory         # "full avg10" = % of time all tasks stalled
```

`oom_kill 0` is decisive: whatever happened, it was not the OOM killer. Do not
report memory exhaustion without that counter being non-zero.

**Orphans wedge the next build.** Killing a `cargo` leaves its `rustc` children
adopted by init. They keep running, can hold locks, and have been seen blocking
a later build in a *different* `CARGO_TARGET_DIR`. If a build wedges right
after you killed a previous one, that is the first thing to check:

```
mcp__cs__coordination action=gc_report
```

Orphaned `rustc` shows up as reapable, annotated "build tool with no parent to
report to".

**Reporting is yours; cleanup is the supervisor's.** `gc_report` is read-only —
run it freely. `gc_cleanup force=true dry_run=false` is **not** scoped to your
worktree: it sweeps every worker's worktree on the host, because all workers run
as the same user. Ask the supervisor rather than running it yourself, and say
which pid you want gone. A worker clearing its own wedge with a host-wide kill
is how one worker's recovery becomes another worker's mystery build failure.

**Never select build processes by name to kill them.** `pkill -9 -f rustc` and
`pgrep -x rustc` match another worker's live compile on a shared host, and you
will destroy their build without knowing — this has actually happened here. The
only pid you may signal directly is one you captured yourself from a process you
started (`$!`), and only after confirming its command line.

Note what the fingerprint does and does not buy you: `gc_cleanup` revalidates a
`/proc` start-time fingerprint before signalling, so it cannot hit a *recycled*
pid — but that proves identity, not that the process is unwanted. It is a
protection against killing the wrong process, not against killing the right
process at the wrong time.
