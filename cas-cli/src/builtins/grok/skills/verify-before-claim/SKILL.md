---
name: verify-before-claim
description: Use immediately before claiming a task, test, build, script, fix, or acceptance criterion is complete.
managed_by: cas
---

# Verify Before You Claim

Immediately before `cas__task action=close`, treat your summary as a hypothesis, not a report: run the proof fresh and capture its result.

## Factory workers: Rust proof

Factory workers never run Rust builds or tests; a PreToolUse guard denies `cargo`, `rustc`, `nextest`, `scripts/run-scoped-tests.sh` and `make test*`. For a Rust change, the worker proof is:

- `git diff --stat` showing exactly the files you expected to change.
- Wiring evidence: `rg '<symbol>'` or `git grep -n '<symbol>'` hits for every new or changed symbol outside its definition.
- Any non-Rust suite the change touches, with its passed count.

Rust tests and builds defer to the supervisor's `ASSEMBLY_PROOF` at epic assembly; write or update the tests, commit them, and say so in the close note. Rows marked "supervisor / non-factory" below do not apply to a factory worker.

## The Four-Step Protocol

Run this every time, immediately before `task action=close`.

### 1. Name the proof command, in plain prose

State (in chat or a task note) the single command that, if it exits zero, proves the work is done. Be specific to this task's claim.

- "AC says the new route returns 200" → proof is `curl -fsS http://localhost:3000/api/foo`.
- "AC says the script accepts `--json`" → proof is `./scripts/export.sh --json | jq .status`.
- "AC says `cargo test --lib pull_scoping` passes" (supervisor / non-factory) → proof is `cargo test --lib pull_scoping`. A factory worker gives the worker proof above instead.

If you cannot name a proof command in one sentence, narrow the claim.

### 2. Run it FRESH in the current worktree

Run it now, after the most recent change, not from memory of an earlier run. For multi-step claims, run each proof command, not just the last one.

```bash
# Run in the current cwd / worktree, not a cached state.
<proof-command>
```

### 3. Capture exit code + tail of output

Show the result to the supervisor, preferably as a task note:

```bash
cas__task action=notes id=<task-id> note_type=progress \
  notes="Proof: <cmd>
Exit: 0
Tail:
<last 5-10 lines of output>"
```

Exit code is the load-bearing line. The tail lets the supervisor check that the command exercised what you think it did (test count, file path, status code).

### 4. Only then, close

If step 3 showed exit 0 (or the documented success signal for non-zero-success commands), call:

```bash
cas__task action=close id=<task-id> reason="<...>"
```

If step 3 showed failure, do not close. Go back to the worker workflow: implement, commit, re-run the proof.

## What Counts As a Proof Command

| Claim type | Proof shape |
|---|---|
| "Tests pass" (factory worker, Rust) | Tests written or updated and committed, plus the worker proof above; the run itself is the supervisor's `ASSEMBLY_PROOF` (see [cas-worker/references/close-gate.md](../cas-worker/references/close-gate.md)) |
| "Tests pass" (non-Rust) | The project's suite (`pnpm test`, `npx vitest run`, `pytest`) with a nonzero passed count |
| "Tests pass" (supervisor / non-factory, Rust) | `cargo test [--lib --test --workspace]` against the relevant scope |
| "Build is clean" (supervisor / non-factory) | `cargo build` for the touched crate(s); `cargo build --workspace` for `pub` type changes |
| "Script runs end-to-end" | The actual script invocation, with the expected input |
| "New CLI subcommand works" | `<binary> <subcommand> [args]` against a freshly built binary (supervisor / non-factory for Rust binaries) |
| "Endpoint returns 200" | `curl -fsS` against a running server, OR an integration test |
| "New code is wired in" | `rg '<symbol>'` or `git grep -n '<symbol>'` showing a caller outside the definition |
| "Diff is clean" | `git diff --stat` showing exactly the files you expected |
| "Specific file/line changed" | `grep -n '<expected-text>' <file>` returning the expected line |
| "Bug repro is gone" | The repro steps run end-to-end, with success-state captured |

A proof command is **observable, deterministic, and recoverable**: anyone re-running it on the same commit gets the same answer. "I ran it earlier" and "It compiled in my IDE" are not proof commands.

## When This Skill Doesn't Fire

- **Pure documentation / markdown-only tasks**: no executable proof exists. Record this explicitly (`note_type=decision`: "No executable proof — documentation-only change. Reviewer must inspect rendered output.") and skip the four steps.
- **Spike / decision tasks** (`task_type=spike`): the deliverable is a decision note, not code. The proof is the existence and content of the decision note — capture that as the proof step instead.
- **Tasks with `execution_note=additive-only`**: ship only new files, verify presence (`ls`, `git status`), and capture that as the proof. The close gate's `additive-only` enforcement (no `M`/`D` lines) is the safety net.

## Advisory vs Required-Paste

This skill is advisory: close does not parse a pasted proof. The mechanical layer is the close gate and the verifier; a supervisor or verifier cites this skill when a close claims done without evidence.

Done when the proof ran after your last change and its result is in a task note.
