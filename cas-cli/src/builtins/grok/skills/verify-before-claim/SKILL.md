---
name: verify-before-claim
description: Use immediately before claiming a task, test, build, script, fix, or acceptance criterion is complete.
managed_by: cas
---

# Verify Before You Claim

You finished the work. You are about to call `cas__task action=close`. Before that — for thirty seconds — invert your trust: act as if your summary is a hypothesis, not a report.

The failure pattern this skill kills:

> "All tests pass." → close → verifier runs `cargo test` → red.
> "Build is clean." → close → CI fails on the file you didn't recompile.
> "Wired up the new route." → close → no handler imports it.

The fix is not more discipline-in-prose. It is a fresh execution of the proof, captured.

## The Four-Step Protocol

Run this every time, immediately before `task action=close`. It is fast; the failure mode it prevents is not.

### 1. Name the proof command, in plain prose

State out loud (in chat or a task note) the single command that, if it exits zero, proves the work is done. Be specific to *this* task's claim.

- "AC says `cargo test --lib pull_scoping` passes" → proof is `cargo test --lib pull_scoping`.
- "AC says the script accepts `--json`" → proof is `./target/release/cas cloud pull --json | jq .status`.
- "AC says the new route returns 200" → proof is `curl -fsS http://localhost:3000/api/foo`.

If you cannot name a proof command in one sentence, you have not narrowed the claim enough. Narrow it.

### 2. Run it FRESH in the current worktree

Not from memory of an earlier run. Not from the build you did before the last edit. Run it *now*, after the most recent change.

```bash
# Run in the current cwd / worktree, not a cached state.
<proof-command>
```

For Rust, that almost always means a `cargo` invocation against the *current* tree. For multi-step claims, run each proof command — not just the last one.

### 3. Capture exit code + tail of output

Display the result to the supervisor (or to yourself). Either inline in chat, or — strongly preferred for non-trivial proofs — as a task note:

```bash
cas__task action=notes id=<task-id> note_type=progress \
  notes="Proof: <cmd>
Exit: 0
Tail:
<last 5-10 lines of output>"
```

Exit code is the load-bearing line. The tail is for the supervisor to spot-check that the command actually exercised what you think it did (test count, file path, status code).

### 4. Only then, close

If — and only if — step 3 showed exit 0 (or the documented success signal for non-zero-success commands), call:

```bash
cas__task action=close id=<task-id> reason="<...>"
```

If step 3 showed failure: do not close. Go back to step 1 of the worker workflow — implement, commit, re-run the proof.

## What Counts As a Proof Command

| Claim type | Proof shape |
|---|---|
| "Tests pass" | `cargo test [--lib --test --workspace]` against the relevant scope (see close-gate.md) |
| "Build is clean" | `cargo build` for the touched crate(s); `cargo build --workspace` for `pub` type changes |
| "Script runs end-to-end" | The actual script invocation, with the expected input |
| "New CLI subcommand works" | `<binary> <subcommand> [args]` against the rebuilt binary |
| "Endpoint returns 200" | `curl -fsS` against a running server, OR an integration test |
| "Diff is clean" | `git diff --stat` showing exactly the files you expected |
| "Specific file/line changed" | `grep -n '<expected-text>' <file>` returning the expected line |
| "Bug repro is gone" | The repro steps run end-to-end, with success-state captured |

A proof command is **observable, deterministic, and recoverable** — anyone re-running it on the same commit gets the same answer. "I ran it earlier" is not a proof command. "It compiled in my IDE" is not a proof command.

## When This Skill Doesn't Fire

- **Pure documentation / markdown-only tasks**: no executable proof exists. Record this explicitly (`note_type=decision`: "No executable proof — documentation-only change. Reviewer must inspect rendered output.") and skip the four steps.
- **Spike / decision tasks** (`task_type=spike`): the deliverable is a decision note, not code. The proof is the existence and content of the decision note — capture that as the proof step instead.
- **Tasks marked `additive-only` with zero behavior change**: ship the file, verify presence (`ls`, `git status`), capture that as the proof. The close gate's `additive-only` enforcement (no `M`/`D` lines) is the real safety net.

## Decision: Advisory vs Required-Paste (v1)

**v1 ships as advisory.** This skill instructs the worker; it does NOT mechanically enforce that a proof-command output is pasted as a task note before close. The reasons:

- The CAS runtime already has `verification_store` + close-gate.md's 6-check self-verification as the mechanical layer. This skill is the *agent-discipline* layer on top.
- Mechanical enforcement (refusing close until a note matching a regex like `Proof:.*\nExit: 0` appears) is straightforward to add later, but adds friction on legitimate documentation/spike tasks and creates a tempting bypass surface ("paste a fake proof to unblock").
- A clear advisory rule that the supervisor can cite when a worker skips proof (and that the verifier prompt can quote when finding "claimed done without evidence") is the right v1 surface. If workers ignore it, escalate to required-paste in v2.

Revisit if telemetry (closed → verifier-reject → still-broken) shows the advisory tier under-performing.

## One-Line Pre-Close Mantra

> Name the proof. Run it fresh. Capture the exit code. THEN close.

If you cannot do all four, you are not done — you are guessing.
