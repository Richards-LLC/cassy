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
   and require the `Scoped Validation (factory/PR)` job inside that branch tip's
   exact-SHA, push-triggered `CI` workflow run to be GREEN. Missing or malformed evidence,
   API errors, pending, skipped, cancelled, or red is a refusal. The only
   substitute is the supervisor running the affected caller modules in the gate
   worktree. The supervisor monitors CI; workers never poll CI.
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
   the last recorded archive size and records the new archive size per run. A
   failure aborts before Cargo or archive rows. Archive mode builds outside the
   checkout with a remap rooted at `Cargo.toml` plus every package path from
   `git ls-files '*/Cargo.toml'`, symlinks only cargo/rustc/git/sh/bash/jq/python3,
   removes `rg`, and uses `--workspace-remap`; exclude `component_output_test`
   only when its source snapshots require the checkout.
8. Before changing a version, prove there is no competing release with
   `gh pr list --state open --search release`, the merge-queue GraphQL query,
   and `git ls-remote --tags origin`. A competing release from another supervisor lands
   first and this train takes the next patch number. Then prepare the source
   commit with `scripts/bump-release-version.sh <version>`,
   `cargo update --workspace --offline`, CHANGELOG, release draft, and prior
   POSTED receipt. Start the gate with `scripts/release-train.sh <version>
   <epic-worktree> --gate`; it regenerates and refuses an uncommitted ledger,
   then launches a nohup detached process group. Schedule a `coordination
   remind`, end the turn, and inspect once with `--status`; never run a shell
   watcher. `--stop` terminates the recorded process group, including nextest
   and git children. After a targeted fix rerun only failed rows with `--gate
   --only <row,row>`; row order and the selected-row summary are preserved.
   These are diagnostic receipts in append-only per-attempt directories: they
   never overwrite or authorize the exact-SHA full gate required by pipeline.
   The main per-run directory is keyed by version and worktree, never only by
   version, and every gate is located by its recorded PID, never `pgrep`.
9. Create `pr-body.md` in the printed run directory, then use `--pipeline`.
   It refuses unless `gate.done`, `gate.full.sha`, and the current tree prove a
   successful full gate on the exact commit about to be pushed. Require
   pull-request Fast Validation and macOS Check pass rows before queue admission;
   skipped push rows prove nothing. The train records the PR, merge-queue
   terminal state, and landed main SHA. It detects a missing `mergeQueueEntry`
   with no new `merge_group` run and re-enqueues at most three times before
   failing; stale queue runs from before this attempt prove nothing.
10. Add one epic note per gate run containing tip, failed rows, cause class
    (`product`, `fixture`, `environment`, or `procedure`), and blocking step.
    The final pane summary names green-to-published latency only from saved,
    verified publication receipts; `--status` is bounded and read-only, prints
    the note template, and otherwise reports publication pending or unavailable.
11. Publish the recorded landed SHA with `--publish`. Require origin/main and
   the landed commit's `cas-cli/Cargo.toml` to match before the detached tag
   worktree is created. Hardlink `.context/zig`, source
   `CAS_RELEASE_ENV_FILE` (default `$HOME/.cas/release.env`) without printing
   values, and print only the set/unset state of `CAS_POSTHOG_API_KEY` and
   `CAS_SENTRY_DSN`. Let `release.sh --publish-tag` create the annotated tag and
   run local preflight. Keep `release.log`, the recorded PID, and numeric done
   receipt in the train's external run directory. A zero publisher status writes
   `release.tag-complete.epoch`, never a publication timestamp: tag push
   completion is separate from GitHub release and asset publication. The detached wrapper must
   capture status without changing the caller's errexit state:

   ```bash
   cd "$TAG_WORKTREE"
   test -x "$PWD/.context/zig/zig"
   export ZIG="$PWD/.context/zig/zig"
   RELEASE_ENV_FILE="${CAS_RELEASE_ENV_FILE:-$HOME/.cas/release.env}"
   test -r "$RELEASE_ENV_FILE"
   set -a; source "$RELEASE_ENV_FILE"; set +a
   for name in CAS_POSTHOG_API_KEY CAS_SENTRY_DSN; do
     if [[ -v "$name" ]]; then printf '%s: set\n' "$name"; else printf '%s: unset\n' "$name"; fi
   done
   EVIDENCE_DIR="$(scripts/release-train.sh "$VERSION" "$EPIC_WORKTREE" --print-run-dir)"
   mkdir -p "$EVIDENCE_DIR"
   LOG="$EVIDENCE_DIR/release.log"
   PID_FILE="$EVIDENCE_DIR/release.pid"; DONE="$EVIDENCE_DIR/release.done"
   nohup bash -c '
     set +e
     "$1" --publish-tag >"$2" 2>&1
     status=$?
     case "$status" in (""|*[!0-9]*) status=1;; esac
     printf "%s\n" "$status" >"$3"
     exit "$status"
   ' bash "$PWD/scripts/release.sh" "$LOG" "$DONE" &
   PUBLISH_PID=$!; printf '%s\n' "$PUBLISH_PID" >"$PID_FILE"
   ```

   At reminder wakeups use `kill -0` on that PID and inspect the done receipt;
   never `wait`, foreground-poll, or use `pkill -f`.
12. Before announcing, require the annotated tag peels to the landed SHA and
   `git ls-remote --exit-code --refs origin "refs/tags/$TAG"` succeeds. The
   release workflow explicitly dispatches `install-path-proof.yml` with
   `version=$TAG` after publication because its release-created `GITHUB_TOKEN`
   does not fan out `release.published`; save that matching `workflow_dispatch`
   run id in the release receipt and require its success. Save the exact
   matching `gh run list --workflow release.yml --limit 20 --json
   databaseId,headBranch,headSha,status,conclusion` row as `release-workflow.json` and
   require success. Save `release-published-receipt.sh "$TAG" --write-draft
   <draft-path>` output as `release-published.receipt` and
   `release-latency-receipt.sh "$TAG"` output as `release-latency.receipt` in
   the run directory. Only the published receipt's matching tag, SHA, actual
   `PUBLISHED_AT`, and both required asset digests authorize `--status` to name
   green-to-published latency. Use
   MechaCassy's default `cas-internal` channel, retain `C0B44GUKDK2` only for
   verification, and save four Slack POSTED entries with timestamps and
   permalinks. If the live proxy lacks registration, use the configured direct
   mecha-cassy MCP or an approved bounded one-shot route; do not retry an
   authenticated-session rejection. Save `cas update`, `cas --version`, and
   `cas hub` proof and require `refresh_binary_version` in the host update JSON
   to equal the released version. Carry the POSTED receipt into the next prep
   commit. Close only after merge and stranded-branch inspection; use
   `stranded_branch_override` only with proof on main.
