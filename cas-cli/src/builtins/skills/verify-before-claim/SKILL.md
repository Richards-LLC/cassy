---
name: verify-before-claim
description: Use immediately before claiming a task, test, build, script, fix, or acceptance criterion is complete.
metadata:
  managed_by: cas
---

# Verify Before You Claim

Immediately before `task action=close`, treat your summary as a hypothesis, not a report. A receipt at the delivered tip is fresh; rerun only after a new commit.

## Claim verdicts

Every claim in a close reason or note carries exactly one verdict:

- **VERIFIED**: the proof ran after your last change and its output is pasted.
- **NOT VERIFIED**: not checked; say why and who owns the check. A factory worker's full suite runs once on the landing tree: the merge queue, else CI, else one supervisor run per epic.
- **INCONCLUSIVE**: checked, but the evidence does not settle it; paste what you saw. Inconclusive is not a pass.

## Proof ladder

Climb as far as the blast radius demands and name the rung you reached: **1. read** (diff, `rg` wiring hits, typecheck) → **2. targeted test** of the changed path → **3. integration or real run** (the actual binary, script, endpoint or suite) → **4. user-path walk** (the journey driven end to end, per `cas-qa-craft`). Docs and one-file local edits stop at rung 1; shared code, config, schema and `pub` API need rung 3; user-facing paths reach rung 4 in batch QA. A `risk=blast-radius` claim below rung 3 on any `proof_targets` entry is NOT VERIFIED.

Test count is never evidence; the one regression that fails without the fix is ([Fewest tests](../cas-writing-tests/SKILL.md#fewest-tests)).

Each claim also states one **safety fact**: what could break and why it did not, backed by a rung ("every `parse_id` caller trims first: `rg -n parse_id` shows three call sites, each after `.trim()`").

## Measurable claims: same-command baseline and treatment

A measurable claim (faster, smaller, fewer, fixed) needs a before and an after from the same command with the same inputs: run it on the base commit, then on your change, and paste both. A delta between two different commands, or against a remembered baseline, is INCONCLUSIVE at best.

## Protocol

1. Name, in a task note, the one command whose success proves the claim: `curl -fsS http://localhost:3000/api/foo` for "the route returns 200", `./scripts/export.sh --json | jq .status` for "the script accepts `--json`". If you cannot name one in a sentence, narrow the claim. Factory workers proving Rust use the capped `check: PASS` and `test: PASS` receipts from [cas-worker/references/discipline.md](../cas-worker/references/discipline.md) instead of raw Cargo.
2. Run each proof command in the current worktree at your latest commit.
3. Record the result:

   ```bash
   task action=notes id=<task-id> note_type=progress \
     notes="Claim: <claim> | Verdict: VERIFIED | Rung: <1-4>
   Proof: <cmd>
   Exit: 0
   Tail:
   <last 5-10 lines of output>
   Safety: <what could break and why it did not>"
   ```

   The exit code is the load-bearing line; the tail shows what the command exercised (test count, file path, status code).
4. Close only when every claim is VERIFIED or NOT VERIFIED with a named owner. A failed or INCONCLUSIVE proof goes back to implement, commit, re-prove.

## Proof shapes

| Claim | Proof |
|---|---|
| Tests pass (factory worker) | Committed tests plus nonzero targeted receipts per [close-gate.md](../cas-worker/references/close-gate.md); the full suite follows the landing-tree owner |
| Tests pass (supervisor / non-factory, Rust) | `cargo test [--lib --test --workspace]` for the relevant scope |
<!-- cas-src:begin -->
| Build is clean (supervisor / non-factory) | `cargo build` for the touched crate(s); `cargo build --workspace` for `pub` type changes |
<!-- cas-src:else -->
| Build is clean (supervisor / non-factory) | The project's build command for the touched packages; `cargo build` (`--workspace` for `pub` type changes) in a Rust project |
<!-- cas-src:end -->
| Script or CLI subcommand works | The real invocation with the expected input, against a freshly built binary |
| Endpoint returns 200 | `curl -fsS` against a running server, or an integration test |
| New code is wired in | `rg '<symbol>'` showing a caller outside the definition |
| Diff is clean | `git diff --stat` showing exactly the files you expected |
| Bug repro is gone | The repro steps end to end, with the success state captured |

A proof is observable and deterministic: anyone re-running it on the same commit gets the same answer.

## Exceptions

- **Standalone documentation**: prove its syntax/render acceptance. When text drives installation, routing or a parser, verify that consumer; never add a wording test. Record unexercised runtime routes explicitly.
- **Spike / decision tasks** (`task_type=spike`): the proof is the decision note's existence and content.
- **`execution_note=additive-only`**: verify presence of the new files (`ls`, `git status`); the close gate refuses modified or deleted files.

Close does not parse a pasted proof; the close gate and the verifier are the mechanical layer, and they cite this skill when a close claims done without evidence.

Done when every claim carries a verdict, a rung and a safety fact, and each proof ran after your last change with its result in a task note.
