# QA candidate missing-builder exit status

**Cassy 3.86.0 already exits 1 when the QA candidate builder is missing**, retaining the existing diagnostic. Task `cas-30ec` therefore delivers bounded proof of current behavior, with no runtime or duplicate test change.

## Real CLI probe

The installed executable reports `cas 3.86.0 (1bee154 2026-10-28)`. A temporary Git checkout under the worker's ignored `target/` contains a valid full revision and an isolated `.cas/config.toml`, with `scripts/qa-candidate-build.py` absent. Its configured artifact destination does not exist before or after the command. The subprocess uses an isolated home/config directory and removes inherited factory identity, so the real operator-facing missing-script path runs without touching the live project.

```sh
cas qa candidate build --task cas-30ec --rev <fixture-full-sha>
cas qa candidate build --task cas-30ec --rev <fixture-full-sha> --json
```

| Mode | Process exit | Standard output | Standard error |
| --- | --- | --- | --- |
| Text | 1 | Empty | Existing missing-builder diagnostic and recovery hint |
| JSON | 1 | Empty | Same diagnostic and recovery hint |

Both runs contain the exact diagnostic prefix:

```text
[ERROR] QA candidate builder missing: scripts/qa-candidate-build.py
Use an up-to-date Cassy source checkout, then rerun:
```

The commands resolve a real commit before reaching the missing-script guard. They do not invoke Python as a builder, compile a candidate or publish a candidate receipt. The task's portable evidence contains `probe-missing-builder.py`, `installed-cli-probe.json` and `probe.log`; rerun the script from a checkout with an ignored `target/` directory. The proof script exits 0 only when the CLI exits nonzero, retains the message, prints no success output and creates no candidate artifacts.

## Dispatch and existing regression

The source at investigation base `ff85adb7c8927ddf61f795cc61a8516462da5cbb` matches the released `v3.86.0` files for all three paths below:

- [`qa.rs`](../../cas-cli/src/cli/qa.rs#L658) rejects an absent builder with `ensure!` before invoking the builder subprocess. `qa::execute` at line 221 and the CLI dispatcher at `cli/mod.rs:1014` propagate that error.
- [`main.rs`](../../cas-cli/src/main.rs#L169) converts the resulting error into `ExitCode::FAILURE` at line 232, after printing its diagnostic.
- [`cli_test.rs`](../../cas-cli/tests/cli_test.rs#L139) already has one missing-builder case inside `qa_candidate_plan_and_receipt_use_project_artifacts_cas_31fab`: it invokes the real CLI, asserts unsuccessful status and later checks the missing-builder message and recovery hint. The fixture uses a small builder substitute for its separate success assertions, never a Rust candidate build.

The regression row was added by `51298473902dc961fb19e953d550867cdb4b9504`; the recovery wording was added by `45d1b7d7b`. Both are ancestors of `v3.73.0`, `v3.86.0` and the investigation base. `source-proof.txt` records exit-0 ancestry checks and unchanged-source comparisons. No new regression row is necessary.

The existing Rust integration row was inspected, not recompiled or rerun for this documentation-only delivery. The fresh installed-binary probes above directly exercise the requested missing-builder process status and message without an unnecessary Rust build.

## Historical limit and acceptance

The report names a stale **v3.62 checkout**, which does not identify the executing CLI binary. The candidate command first appears in `4686e1ab5`, contained in `v3.71.0`; `cas-cli/src/cli/qa.rs` does not exist in the `v3.62.0` tag. Without the original binary identity and exit capture, the reported exit-0 cause remains **INCONCLUSIVE**. No claim is made that a particular intervening commit fixed that historical process.

| Acceptance | Verdict |
| --- | --- |
| Missing-builder process returns nonzero | VERIFIED: two installed-CLI probes return 1 |
| Existing message remains visible | VERIFIED: exact prefix and recovery hint in captured stderr |
| One regression row | VERIFIED by source inspection: existing real-CLI case at `cli_test.rs:139`, retained since 3.73 |
| No candidate build/profile/assembly changes | VERIFIED: documentation-only diff; probes create no candidate artifacts |
| Historical v3.62-checkout exit-0 cause | INCONCLUSIVE: original executing binary and capture unavailable |
| Browser/terminal rendering journey | NOT A ROUTE: existing output unchanged |

Validation: the standalone probe exits 0 with both CLI refusals captured; Markdownlint and `git diff --check` validate this document. Full integrated execution remains deferred to the merge queue. Design: `DESIGN.md@b43940e43`, plain Markdown with the project's factual voice.
