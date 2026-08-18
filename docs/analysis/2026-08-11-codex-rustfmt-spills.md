# Codex worker rustfmt spills: root-cause investigation

## Verdict

The spills are caused by model-issued formatter commands, not a Codex auto-save
or hidden tool default. Confidence is **high**. Two mutating command shapes cause
the same symptom:

1. `cargo fmt` selects Cargo targets, so `cargo fmt --all` rewrites the workspace.
2. Direct `rustfmt` follows `mod` declarations by default, so naming a module root
   such as `cas-cli/src/cli/mod.rs` or `crates/cas-store/src/lib.rs` also formats
   child modules that were not named on the command line.

The 2026-08-11 rollout contains both triggers back-to-back, each followed by an
unrelated-file spill. The second mechanism explains the previously unattributed
`cas-bb1c` incident: its task-owned files included `crates/cas-store/src/lib.rs`,
and the worker reported running targeted `rustfmt` on its task files before about
19 `cas-store` files changed.

## Overview

| Field | Result |
| --- | --- |
| Question | What repeatedly rewrites unrelated Rust files in Codex worker worktrees? |
| Verdict | Explicit workspace Cargo formatting and recursive direct rustfmt module traversal |
| Confidence | High for the causal mechanisms and the 2026-08-11 incident; medium-high for the retrospective `cas-bb1c` attribution |
| Scope examined | Three task incident notes; the complete eager-leopard-72 Codex rollout; formatter commands across stored 2026-08-09 through 2026-08-11 rollouts; repository Codex hooks and Cassy PreToolUse policy |
| Date | 2026-08-11 |
| Author | Cassy factory worker `warm-cheetah-6`, task `cas-852a0` |

## Evidence

| Observation | Source | What it proves |
| --- | --- | --- |
| At 13:07:57Z the model invoked `cargo fmt --all && git diff --check && git diff --stat && git status --short`. | Eager-leopard-72 rollout, response ordinal 130 | The workspace formatter was explicitly requested by the model; it was not an implicit tool action. |
| The command completed at 13:08:00Z and immediately printed formatting changes across benches and many unrelated `cas-cli` modules. | Same rollout, command-execution ordinal 131 | `cargo fmt --all` produced the first spill. |
| At 13:08:19Z the worker restored every changed path outside three task-owned tracked files. | Same rollout, ordinals 146–147 | The first spill was removed before the next formatter experiment. |
| At 13:08:23Z the model invoked direct `rustfmt` on seven named files, including `cas-cli/src/cli/mod.rs`, `crates/cas-core/src/lib.rs`, and `crates/cas-core/src/sync/mod.rs`. | Same rollout, response ordinal 152 | The second experiment appeared file-scoped but included module roots. |
| One second later, `git status --short` listed unrelated children such as `cas-cli/src/cli/auth.rs`, `cloud.rs`, `doctor.rs`, `factory/mod.rs`, and many more. | Same rollout, command-execution ordinal 153 | Direct rustfmt recursively followed module declarations and produced a second spill without `cargo fmt` or an external writer. |
| At 13:08:30Z the worker again restored every non-task path. | Same rollout, ordinals 158–160 | The child-module spill was distinct from the earlier workspace spill. |
| The `cas-bb1c` worker stated it ran targeted `rustfmt --edition 2024` on task files; commit `d5e2ef4b` includes `crates/cas-store/src/lib.rs`; about 19 unrelated `cas-store` files then changed. | Cassy task `cas-bb1c` notes and `git show --name-only d5e2ef4b` | The historical file family and timing match rustfmt traversal from the crate module root. This is a retrospective inference because that original rollout is not present in the retained Codex session directory. |
| Before this change, `.codex/hooks.json` registered only `PostToolUse`, which cannot prevent writes. | Repository `.codex/hooks.json` at investigation start | Cassy already had worker safety logic, but Codex unified exec was not routed through it before execution. |

## Reasoning chain

The 2026-08-11 chronology is a controlled A/B sequence inside one worktree. The
worker starts with task-only edits, runs `cargo fmt --all`, observes broad churn,
and restores it. The worker then runs direct `rustfmt` on a list of apparent task
files, observes a different broad set rooted under the named modules, and restores
that too. Because the worktree was clean of the first spill before the second
command, the second result cannot be residue from Cargo formatting.

Rustfmt's module traversal connects the second command to its output: a root file
containing `mod child;` causes the child source to be formatted unless
`skip_children=true` is supplied. That also reconciles the `cas-bb1c` report. The
worker accurately remembered naming only task files, but one task file was the
crate's `lib.rs`; “targeted by path” did not mean “limited to that path.”

The evidence rules out the candidate explanations as follows:

- **Codex auto-format on save:** ruled out for the reproduced incident. Each spill
  begins at an explicit shell tool call, and the rollout shows no formatter write
  before those calls.
- **Tool default unrelated to rustfmt:** ruled out. The command-execution records
  preserve the exact commands and their immediate Git output.
- **`cargo fmt --check` mutating files:** ruled out. The retained rollout corpus
  contains many check invocations without write evidence; the confirmed writes
  follow mutating `cargo fmt --all` and mutating direct `rustfmt`.
- **Another concurrent writer:** ruled out for the warm incident by the restore →
  direct rustfmt → immediate status sequence. It remains theoretically possible
  for the first historical incident because its original rollout was not retained.

## Timeline

| UTC | Event | State |
| --- | --- | --- |
| 13:07:57 | Model requests `cargo fmt --all`. | First trigger |
| 13:08:00 | Git output shows workspace-wide formatting churn. | First spill detected |
| 13:08:19 | Unrelated paths restored. | Task-only state restored |
| 13:08:23 | Model requests direct rustfmt on task files including module roots. | Second trigger |
| 13:08:24 | Git output shows many unrelated child modules. | Second spill detected |
| 13:08:30 | Unrelated paths restored again. | Task-only state restored |
| 13:08:44 | Worker records the accidental formatter rewrite in the task note. | Incident reported |

## What would falsify this

The conclusion should be revisited if a clean worktree gains formatting-only
changes before any formatter process starts, or if direct rustfmt with
`--config skip_children=true` changes an unlisted child module. Process-level
evidence showing another writer modifying the same paths during the one-second
command windows would also overturn the single-writer attribution.

## Next actions

1. **Implement now — worker guard/config.** Route Codex unified exec through
   `cas hook PreToolUse`. For factory workers, deny mutating `cargo fmt` and deny
   mutating direct rustfmt unless `skip_children=true` is explicit. Continue to
   allow `cargo fmt ... --check`, direct rustfmt checks, and stdout-only output.
   Supervisors retain normalization authority. The official
   [Codex Hooks documentation](https://learn.chatgpt.com/codex/hooks) confirms
   that unified exec is exposed to `PreToolUse` as `Bash` and that hook decisions
   apply to nested code-mode tool calls.
2. **Verify the guard.** Unit-test compound commands, toolchain-qualified Cargo,
   absolute tool paths, recursive module-root calls, safe checks, safe explicit
   `skip_children=true`, and supervisor exemption. Also assert that the project
   Codex hook configuration contains the `PreToolUse` route and that fresh
   `cas init/update` configuration installs both Cassy pre- and post-tool hooks
   without replacing custom hooks.
3. **Operator decision — do not execute in this task.** A one-time workspace
   `cargo fmt --all` normalization followed immediately by a CI
   `cargo fmt --all -- --check` gate would remove the chronic baseline drift and
   make future formatting checks meaningful. It would also create a large
   blame-noise commit. Run it only as an isolated, announced operator-owned change.
   Normalization reduces blast radius but does not replace the worker scope guard.

## Provenance

- Markdown source: `docs/analysis/2026-08-11-codex-rustfmt-spills.md`
- HTML review surface: `docs/analysis/2026-08-11-codex-rustfmt-spills.html`
- Repository branch at analysis time: `factory/warm-cheetah-6`
- Warm-incident base commit: `f96b5831`
- Historical task commits examined: `006131d7`, `d5e2ef4b`
- Rollout data window: 2026-08-09 through 2026-08-11
- Primary rollout:
  `/home/pippenz/.codex/sessions/2026/08/11/rollout-2026-08-11T09-03-46-019ff0ec-01d1-7193-99a0-0f012adab55c.jsonl`
- Extraction commands:

  ```text
  jq -c 'select(.type=="response_item" and (.payload.type=="function_call" or .payload.type=="custom_tool_call")) | {timestamp,ordinal,kind:.payload.type,name:.payload.name,arguments:.payload.arguments,input:.payload.input}' <rollout>

  jq -r 'select(.type=="event_msg" and .payload.type=="item_completed" and .payload.item.type=="CommandExecution" and (.ordinal>=130 and .ordinal<=160)) | "ORD=\(.ordinal) TIME=\(.timestamp)\nCMD=\(.payload.item.command[-1])\nOUT=\(.payload.item.stdout)"' <rollout>

  git show --name-only d5e2ef4b1de2eadbd8a613ebd8da62b6948b6921
  ```

The HTML companion is a presentation of this Markdown source; the Markdown is
the source of truth.
