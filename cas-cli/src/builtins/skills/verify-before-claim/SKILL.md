---
name: verify-before-claim
description: Use immediately before claiming a task, test, build, script, fix, or acceptance criterion is complete.
metadata:
  managed_by: cas
---

# Verify Before You Claim

Immediately before `task action=close`, treat your summary as a hypothesis, not a report: run the proof fresh and capture its result.

## Claim verdicts

Every claim in a close reason or note carries exactly one verdict; "looks good", "should work" and "it compiled in my IDE" are not verdicts.

- **VERIFIED**: the proof ran after your last change and its output is pasted.
- **NOT VERIFIED**: not checked; say why and who checks it (for example the supervisor's `ASSEMBLY_PROOF`).
- **INCONCLUSIVE**: checked, but the evidence does not settle it; paste what you saw. Inconclusive is not a pass.

## Proof ladder

Climb as far as the blast radius demands and name the rung you reached: **1. read** (diff, `rg` wiring hits, typecheck) → **2. targeted test** of the changed path → **3. integration or real run** (the actual binary, script, endpoint or suite) → **4. user-path walk** (the journey driven end to end, per `cas-qa-craft`). Docs and one-file local edits stop at rung 1; shared code, config, schema and `pub` API need rung 3; user-facing paths need rung 4. A `risk=blast-radius` claim below rung 3 on any `proof_targets` entry is NOT VERIFIED.

Each claim also states one **safety fact**: what could break and why it did not, backed by a rung ("every `parse_id` caller trims first: `rg -n parse_id` shows three call sites, each after `.trim()`").

## Measurable claims: same-command baseline and treatment

A measurable claim (faster, smaller, fewer, fixed) needs a before and an after from the **same command** with the same inputs: run it on the base commit, then on your change, and paste both. A delta between two different commands, or against a remembered baseline, is not attributable; it is INCONCLUSIVE at best.

## Factory workers: Rust proof

Factory workers never run Rust builds or tests; a PreToolUse guard denies `cargo`, `rustc`, `nextest`, `scripts/run-scoped-tests.sh` and `make test*`. For a Rust change, the worker proof is:

- `git diff --stat` showing exactly the files you expected to change.
- Wiring evidence: `rg '<symbol>'` or `git grep -n '<symbol>'` hits for every new or changed symbol outside its definition.
- Any non-Rust suite the change touches, with its passed count.

Rust tests and builds defer to the supervisor's `ASSEMBLY_PROOF` at epic assembly; write or update the tests, commit them, and mark the run NOT VERIFIED (owner: `ASSEMBLY_PROOF`) in the close note. Rows marked "supervisor / non-factory" below do not apply to a factory worker.

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
task action=notes id=<task-id> note_type=progress \
  notes="Claim: <claim> | Verdict: VERIFIED | Rung: <1-4>
Proof: <cmd>
Exit: 0
Tail:
<last 5-10 lines of output>
Safety: <what could break and why it did not>"
```

Exit code is the load-bearing line. The tail lets the supervisor check that the command exercised what you think it did (test count, file path, status code).

### 4. Only then, close

If every claim is VERIFIED (exit 0, or the documented success signal for non-zero-success commands), or NOT VERIFIED with a named owner, call:

```bash
task action=close id=<task-id> reason="<...>"
```

If a proof failed or a claim is INCONCLUSIVE, do not close. Go back to the worker workflow: implement, commit, re-run the proof.

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

A proof command is **observable, deterministic, and recoverable**: anyone re-running it on the same commit gets the same answer. "I ran it earlier" is not a proof command.

## When This Skill Doesn't Fire

- **Pure documentation / markdown-only tasks**: no executable proof exists. Record this explicitly (`note_type=decision`: "No executable proof — documentation-only change. Reviewer must inspect rendered output.") and skip the four steps.
- **Spike / decision tasks** (`task_type=spike`): the deliverable is a decision note, not code. The proof is the existence and content of the decision note — capture that as the proof step instead.
- **Tasks with `execution_note=additive-only`**: ship only new files, verify presence (`ls`, `git status`), and capture that as the proof. The close gate's `additive-only` enforcement (no `M`/`D` lines) is the safety net.

## Advisory vs Required-Paste

This skill is advisory: close does not parse a pasted proof. The mechanical layer is the close gate and the verifier; a supervisor or verifier cites this skill when a close claims done without evidence.

Done when every claim carries a verdict, a rung and a safety fact, and each proof ran after your last change with its result in a task note.
