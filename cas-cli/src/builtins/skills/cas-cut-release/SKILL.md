---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic; follow this fail-closed train.
managed_by: cas
---

# Cassy release train

This is the only supervisor release procedure. Stop at the first failed step,
name the blocking step in the operator timeline, and require its receipt.

1. Read references/failure-log.md in full. Learn an absent failure with
   `scripts/release-gate.sh --learn "<symptom>" "<cause>" "<check-id>"`, add
   its executable row and self-test fixture in the same commit, and store the
   same text with `mcp__cas__memory action=remember entry_type=learning
   tags=release`. `--learn` regenerates the builtin reference ledger.
2. Before `worktree_merge` on any release-bound lane, run
   `scripts/release-train.sh <version> <epic-worktree> --check-lane <branch>`
   and require that branch tip's own push-triggered Scoped Validation to be
   GREEN. Missing, pending, skipped, cancelled, or red is a refusal. The only
   substitute is the supervisor running the affected caller modules in the
   gate worktree. The supervisor monitors CI; workers never poll CI.
3. A stalled worker with green proofs gets one urgent interrupt; if it remains
   stalled, the supervisor pushes from its worktree. When a rebase makes the
   anchor stale, close the handoff with `commit_receipt` instead of spending
   another worker turn rebasing it.
4. Assemble the exact epic tip in a dedicated worktree. Reconcile sibling lanes
   there, run the full suite on the assembled tree, inspect guardrail/marker and
   counted-field tests, and commit every trim or move. Real-project fixtures use
   `cas::test_paths::runtime_fixture_parent()`, never `/tmp`, `/var/tmp`, or
   `env!("CARGO_MANIFEST_DIR")`. `cas init` and serve registration remain
   unconditional; only discovery/cloud behavior skips disposable roots.
5. Treat an intentional doctor row change as a reviewed snapshot update in the
   prep commit and name the row in that commit message; an unexplained snapshot
   change is a bug. Fixture versions use the unmistakable `9.99.x` range,
   never a plausible current or next release such as an `-rc.1` value.
6. After the final merge and after every `--learn`, regenerate
   `cas-cli/src/builtins/reference-history.json` and commit it before the gate.
   The ledger is the last prep step. For builtin agent changes, update root
   projections and run the flavor/projection drift tests; do not use
   `cas update --sync` in the source worktree.
7. Configure `CAS_RELEASE_GATE_HOME_DIR` on the release host to a large base on
   the checkout mount with a writable parent and no `.cas` ancestor. On
   soundwave use `/home/cas-release-gate/base`, not `/`, which was 97% full.
   The scratch-base row runs first and requires free space at least twice
   the last recorded archive size and records the new archive size per run.
8. Prepare the source commit with `scripts/bump-release-version.sh <version>`,
   `cargo update --workspace --offline`, CHANGELOG, release draft, and prior
   POSTED receipt. Start the gate with `scripts/release-train.sh <version>
   <epic-worktree> --gate`; it regenerates and refuses an uncommitted ledger,
   then launches a nohup detached process group. Schedule a `coordination
   remind`, end the turn, and inspect once with `--status`; never run a shell
   watcher. `--stop` terminates the recorded process group, including nextest
   and git children. After a targeted fix rerun only failed rows with `--gate
   --only <row,row>`; row order and the selected-row summary are preserved.
9. Create `pr-body.md` in the printed run directory, then use `--pipeline`.
   Require pull-request Fast Validation and macOS Check pass rows before queue
   admission; skipped push rows prove nothing. The train records the PR, merge
   queue terminal state, and landed main SHA in the same per-run directory,
   which is keyed by version and worktree. Locate processes only by recorded
   PID/process group, never by `pgrep` or a version-keyed path.
10. Add one epic note per gate run containing tip, failed rows, cause class
    (`product`, `fixture`, `environment`, or `procedure`), and blocking step.
    The final pane summary names green-to-published latency; `--status` prints
    the note template and latency receipt when available.
11. Publish the recorded landed SHA with `--publish`. Require origin/main and
    the landed commit's `cas-cli/Cargo.toml` to match before the detached tag
    worktree is created. Let `release.sh --publish-tag` create the annotated tag
    and run local preflight; keep log, PID, and numeric done receipts outside
    the tag worktree. At reminder wakeups use `kill -0` on the recorded PID and
    inspect the done receipt; never foreground-poll or use `pkill -f`.
12. Before announcing, verify the remote tag and release workflow, run
    `release-published-receipt.sh --write-draft` and the latency receipt, save
    the required Slack POSTED receipts, and prove host `cas update`, `cas
    --version`, and `cas hub`. Close only after merge and stranded-branch
    inspection; use `stranded_branch_override` only with proof on main.
