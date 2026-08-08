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
cas__coordination action=server_start command="npm run dev" cwd=<dir> port=3000
cas__coordination action=server_list
cas__coordination action=server_stop id=<id>
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
cas__coordination action=remind target=<your-name> remind_delay_secs=600 \
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
- `cargo test --lib <module>` / `cargo test <name-substring>` — one module or one test
- `cargo test --test <name>` — one integration-test file
- `cargo test -p <crate>` — one crate

Reserve the full suite for close gates on shared/public surfaces — and background it.

### The test loop: inner loop vs final proof

Most multi-fix tasks are lost here, not in the thinking. The failure looks like this, observed
live: a worker fixing several test entry points ran the full `cargo test -p <crate> --lib`
sweep (~3,700 tests, ~5 minutes) after **each individual fix**, foreground-`sleep`ing between
checks. 47+ minutes of wall-clock, almost all of it waiting, for maybe 4 minutes of edits.

Two loops, and they are not the same loop:

**Inner loop — seconds, run constantly.** Targeted filters only: the module, the test name, the
one integration binary. This is where you iterate. If your inner loop takes minutes, it is not
an inner loop — narrow the filter further.

**Final proof — minutes, run at most twice.** The full scoped suite runs once after you have
landed the whole batch of fixes, and once more as the pre-close receipt. That is the budget.
A third full run means you skipped the batching step.

The rules that follow from that:

1. **Batch before you verify.** When you find three broken call sites, fix all three, then run.
   Do not fix-run-fix-run. Each unnecessary full run costs you ~5 minutes and buys information
   you were about to get anyway.
2. **Reuse a banked receipt.** If a full sweep already passed at the commit you are closing on
   and your later edits are provably outside its blast radius, cite it — do not re-run it to
   feel better. Say which commit it was taken at and what it covered.
3. **`cargo nextest run` when it is installed** (`cargo nextest run --lib <filter>`) — it runs
   test binaries in parallel and fails fast, which is often several times quicker than
   `cargo test` on a suite this size. Check once with `cargo nextest --version`; if it is
   missing, fall back to `cargo test` and do not spend the task installing it.
4. **Never foreground-`sleep` waiting on a run.** Background it (Recipe 1) and spend the
   minutes on other deliverable work: the next fix, the task note, the close-gate checks, the
   PR body. A worker asleep in the foreground cannot even receive a stand-down order.
5. **Arm the relevant guard in the inner loop**, not just in the final sweep. A guard that only
   runs in the 5-minute sweep teaches you nothing for 5 minutes.

A worked shape, start to close: targeted filters while fixing → one full scoped sweep after the
batch, backgrounded → other work while it cooks → close on that receipt, or on a banked sweep
plus a targeted run covering exactly what changed since.

### A green exit code is not a green test run

Exit code 0 means "nothing reported failure". It does **not** mean tests ran. Three runs in
this repo exited 0 while executing **zero tests**, all on one day, all judged green by their
author (GH #173):

1. `cargo test -p cas-cli --lib <filter>` — the crate is named `cas`, not `cas-cli`. cargo
   errored; the compound command around it swallowed the status.
2. `cargo test -p cas --lib <filter>` with a **relative** `$ZIG` — ghostty_vt_sys's build
   script panicked. Build scripts run with cwd set to the *crate* directory, so a relative
   `.context/zig/zig` resolves against the wrong root. A worktree does not have a `.context`
   at all; the toolchain lives in the main checkout.
3. `cargo test -p cas --lib some_module::tests::` where the module had been renamed to
   `some_module::additive_only_tests::`. Output, verbatim: `test result: ok. 0 passed;
   0 failed; 0 ignored; 0 measured; 3929 filtered out`. Exit 0.

Run your final-proof suite through the guard, which enforces all of that mechanically:

```bash
make -C cas-cli test-scoped SCOPED_ARGS='-p cas --lib my_module'
scripts/run-scoped-tests.sh -p cas --test cli_test        # same thing, directly
```

It fails the run unless cargo exited 0, **a test harness actually reported**, and the passed
count is greater than zero. The middle condition is the one that matters: a wrapper or a
pipeline can drop a nonzero status, but nothing can invent a `test result:` line. It also
rejects a relative `$ZIG` before spending a build on it, and reads `cargo nextest run`'s
`Summary` line as well as `cargo test`'s.

Then quote the number. **"Tests pass" is not a receipt; "210 passed; 0 failed" is.** Read the
`test result:` line yourself and put its counts in your close note. A passed count of 0 is a
failure to run — never report it as green, however cheerfully cargo prints `ok`.

### Running tests in the clean-CI environment shape

Your shell exports ~15 `CAS_*` identity variables. A test that reads one passes for you and
fails only on a clean CI runner. That is not hypothetical: GH #136's tests shipped red exactly
this way — they resolved a supervisor name from the ambient `CAS_SUPERVISOR_NAME` and passed
100% of the time in every factory shell.

```bash
make -C cas-cli test-clean-env                                     # one binary, clean env
make -C cas-cli test-clean-env CLEAN_ENV_ARGS='--lib cloud::config'
```

It enumerates `CAS_*` from your live environment, prints what it stripped, and runs the scoped
tests without them. **Use it for scoped runs too**, not just full-suite gates, whenever your
diff touches agent resolution, coordination, messaging, cloud config, or anything else that
reads the environment. Scoped is where this bites: the GH #136 tests were only ever run with
`--test factory_mcp_ops_test`, so a full-suite-only rule would not have caught them.

Do not hand-maintain an `env -u ...` list. The one that used to live here had drifted in both
directions — it missed `CAS_CLOUD_TOKEN` and `CAS_CLOUD_ENDPOINT` (a "sanitized" run could
still reach the real cloud) and stripped two variables that no longer exist. `CAS_ROOT` and
`CAS_CLONE_PATH` matter most: left set, a test can reach the *main* checkout's `.cas`. There is
no `CAS_TASK_ID`; don't add it back.

### If you are already blocked

You cannot message from inside a blocked turn. Once the command finally returns, run
`cas__coordination action=inbox_poll` **first**, before anything else, to pull the
messages that could not reach you — and keep polling until it says no unread messages. Then
honor anything that superseded your work: a stand-down order you answer 40 minutes late is
still an order, and a scope change you missed means your last hour needs redoing, not merging.

## Part 2 — Context budget discipline

Context is a consumable the operator pays for. Auto-compaction is not a safety net — it is the
expensive failure mode. Budget it the way you budget wall-clock.

### Report headroom in every milestone note

Every `note_type=progress` milestone note ends with your remaining context, plainly:

```
cas__task action=notes id=<task-id> note_type=progress \
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
   cas__task action=notes id=<task-id> note_type=progress \
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
