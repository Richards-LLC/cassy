# Operating Discipline — Staying Reachable and Staying Alive

Proactive habits that keep a worker useful for a whole shift. The other references are about
moments (closing, breaking, looking things up); this one is about how you run continuously.
Two failure modes cost real money on 2026-08-06 (GH #121) and both are fully preventable:

- **Blocked pane** — a worker foreground-watched a queued CI run for 40+ minutes through a
  provider outage. Nine messages, including two stand-down orders, could not reach it. The
  operator had to kill it.
- **Working into auto-compaction** — the same worker, and a second one in the same window,
  ran their context to the wall and were killed mid-compaction. The operator paid for
  re-summarizing work that a `git push` would have preserved for free.

## Part 1 — Never block the pane

A foreground command owns your turn until it exits. Messages are delivered *between* turns,
so while it runs you are unreachable — there is no recovery from inside a blocked turn, only
prevention.

### The 2-minute rule

If a command *can* exceed ~2 minutes, it does not run in the foreground. That includes:

- builds (`cargo build`, `cargo test` link steps, `pnpm build`, Docker builds)
- full test suites
- deploys and release pipelines
- anything that listens on a port
- **every CI wait** — `gh run watch`, `gh pr checks --watch`, sleep/poll loops

"It usually finishes in 30 seconds" is not an exemption. The failure mode is the tail, and
the tail is where outages, queues and hangs live.

Two sanctioned shapes: **background it** and end your turn (or keep working), or **replace
the wait with a reminder** and end your turn.

### Recipe 1 — builds and test suites

Run it detached and return to the pane immediately. If your harness exposes a background-run
affordance (Claude Code's Bash `run_in_background`), use it; otherwise redirect and detach:

```bash
cargo test --lib > /tmp/cas-test.log 2>&1 &
```

Then end your turn or do unrelated work. When you come back, tail the log and report the exit
status — never claim a pass you did not read:

```bash
tail -40 /tmp/cas-test.log
```

Do not sit in a `wait`/`sleep` loop watching it. Waiting in the foreground for a backgrounded
job re-creates exactly the problem backgrounding solved.

### Recipe 2 — servers and anything that listens

Never `npm run dev &` by hand: a raw background process dies with your worker teardown or,
worse, outlives it unowned. Use the CAS server registry — the only supported way to keep a
server alive across worker lifetime:

```
mcp__cas__coordination action=server_start command="npm run dev" cwd=<dir> port=3000
mcp__cas__coordination action=server_list
mcp__cas__coordination action=server_stop id=<id>
```

`server_list` reports the ports actually bound and who started them. Use `shared=true` only
for services that must survive your teardown.

### Recipe 3 — CI waits (foreground `gh run watch` is BANNED)

There is no acceptable foreground CI wait. Not `gh run watch`, not `gh pr checks --watch`,
not a hand-rolled poll loop, not "just this once because the run is already green-ish".

The sanctioned pattern is **queue the rerun → set a reminder → end the turn → go idle**:

```bash
git push                                   # or: gh workflow run <wf> --ref <branch>
gh run list --branch <branch> --limit 1    # one-shot: confirm it was queued, then stop
```

```
mcp__cas__coordination action=remind target=<your-name> remind_delay_secs=600 \
  remind_message="Check CI on <branch>: gh run list --branch <branch> --limit 3"
```

Then end your turn. The reminder arrives as an injected turn; act on it, run one-shot
`gh run list` / `gh run view --log-failed`, and re-arm another reminder if the run is still in
flight. Each check is a fresh short turn, so the supervisor can reach you between them.

If CI is queued behind an outage or a long backlog, say so in a progress note and re-arm with
a longer delay — do not convert waiting into watching.

### Recipe 4 — anything else that might hang

Bound it explicitly rather than hoping: `timeout 120 <cmd>`, `--max-time` for `curl`,
`--no-watch`/`--run` for test runners that default to watch mode. If you cannot bound it,
background it.

### Scoped tests

Scope before you run. A full suite in this repo links dozens of test binaries; a targeted
change rarely needs it:

- `cargo test --lib` — library unit tests only
- `cargo test --test <name>` — one integration-test file
- `cargo test -p <crate>` — one crate

Reserve the full suite for close gates on shared/public surfaces — and background it.

### Running the full test suite in a worker

Factory identity variables inherited by worker processes can change test behavior. Use this
canonical sanitized command for full-suite gates; do not shorten the list:

```bash
env -u CAS_AGENT_ROLE -u CAS_AGENT_NAME -u CAS_AGENT_ID -u CAS_FACTORY_MODE \
    -u CAS_FACTORY_SESSION -u CAS_FACTORY_SUPERVISOR_CLI -u CAS_FACTORY_WORKER_CLI \
    -u CAS_SESSION_ID -u CAS_SUPERVISOR_NAME -u CAS_CLONE_PATH -u CAS_ROOT \
    cargo test --no-fail-fast
```

That is what the factory actually injects. `CAS_ROOT`/`CAS_CLONE_PATH` matter most — left set,
a test can reach the *main* checkout's `.cas`. There is no `CAS_TASK_ID`; don't add it back.

### If you are already blocked

You cannot message from inside a blocked turn. Once the command finally returns, run
`mcp__cas__coordination action=inbox_poll` **first**, before anything else, to pull the
messages that could not reach you — and keep polling until it says no unread messages. Then
honor anything that superseded your work: a stand-down order you answer 40 minutes late is
still an order, and a scope change you missed means your last hour needs redoing, not merging.

## Part 2 — Context budget discipline

Context is a consumable the operator pays for. Auto-compaction is not a safety net — it is the
expensive failure mode. Budget it the way you budget wall-clock.

### Report headroom in every milestone note

Every `note_type=progress` milestone note ends with your remaining context, plainly:

```
mcp__cas__task action=notes id=<task-id> note_type=progress \
  notes="Migration + tests written, suite green. Context: ~45% used."
```

An estimate is fine; the point is the *trend*. The supervisor cannot see your context, so a
note without it hides the one signal that predicts a mid-task death. A worker that goes from
40% to 75% in one step is about to need a checkpoint, and the supervisor can only stage a
replacement if it can see that coming.

### Checkpoint before compaction — never work through it

When context is running low (roughly 70–75% used, or sooner if the remaining work is large),
**stop feature work and checkpoint**. Four steps, in order:

1. `git add -A && git commit` — commit everything, even partial work, with an honest message.
2. `git push` — an unpushed checkpoint is not a checkpoint.
3. Write a structured handoff note on the task: current state, exact next step, gotchas found,
   commands already proven and their results.
   ```
   mcp__cas__task action=notes id=<task-id> note_type=progress \
     notes="CHECKPOINT. State: <what is done + tip SHA>. Next: <exact next step>. \
            Gotchas: <traps found>. Proven: <command + result>. Context: ~75% used."
   ```
4. Message the supervisor asking for a respawn, naming the branch and tip SHA.

Then stop. A fresh worker resuming from a pushed checkpoint is dramatically cheaper than
compaction plus a degraded continuation — and far cheaper than being killed mid-compaction
with uncommitted work. Never let a factory task run into auto-compaction.

If you are already degrading — garbled output, repeating a fix you already made, losing the
thread of your own plan — that is context exhaustion, and you cannot self-recover. Checkpoint
now and see [recovery.md](recovery.md).

### Right-size your commits

Prefer many small pushed commits over one large uncommitted WIP. Commit and push after each
logical unit, not at the end. The cost of any checkpoint, kill, respawn or crash is exactly
the work since your last push — keep that measured in minutes, not hours. Small commits also
make the supervisor's merges reviewable and let a replacement worker see where you got to.
