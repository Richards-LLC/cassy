---
name: cas-cut-release
description: Use when cutting a Cassy runtime release from an assembled epic; follow this fail-closed train.
managed_by: cas
---

# Cassy release train

This is the only supervisor release procedure. Every numbered step is a gate:
stop at its first failure, record the blocking step in the operator timeline,
and do not claim success without its receipt.

1. Read references/failure-log.md in full. If a new failure is not logged,
   learn it with `scripts/release-gate.sh --learn "<symptom>" "<cause>"
   "<check-id>"`, add its executable check and `scripts/test-release-gate.sh`
   fixture in the same commit, then record the same text with
   `mcp__cs__memory action=remember entry_type=learning tags=release`.
2. Assemble the exact epic tip: `git worktree add .cas/epic-<id>-merge
   <epic-tip>`. Reconcile sibling lanes there before the prep commit: run the
   full suite, inspect guardrail/marker and counted-field tests (grep
   `conflicts_resolved` and assert directional fields), and commit any trim
   or move.
3. After builtin changes, copy each changed embedded agent file from
   `cas-cli/src/builtins/**/agents` to its root projection (`.claude/agents`,
   `.codex/agents`). Do not run `cas update --sync`; run the root-managed-
   projections drift test, regenerate `reference-history.json`, and require
   `git diff --quiet -- cas-cli/src/builtins/reference-history.json`.
4. Prepare one source commit: run `scripts/bump-release-version.sh <version>`,
   `cargo update --workspace --offline`, update CHANGELOG, the runtime-release
   draft, and the previous POSTED receipt; then run
   `scripts/release-gate.sh <version>` and require every row PASS.
5. For archive mode, the gate already puts the archive and TMPDIR under
   `/var/tmp/cas-release-gate` — a large real-disk scratch base with no `.cas`
   ancestor, never `/tmp` tmpfs. Set `CAS_RELEASE_GATE_HOME_DIR` only to move
   that base; the receipt names the base and where it came from, and the gate
   still refuses any base with a `.cas` ancestor. Build a remap
   with root `Cargo.toml` and every package path from `git ls-files
   '*/Cargo.toml'`; run outside the checkout with symlinks for
   cargo/rustc/git/sh/bash/jq/python3, no `rg`, and `--workspace-remap`.
   Exclude `component_output_test` only when its source snapshots need checkout.
6. Push one source branch and open one protected-main PR:
   `git push -u origin <source-branch>`; `gh pr create --base main --head
   <source-branch> --fill`; `gh pr merge <pr> --merge`. Verify the PR remains
   queued, explicitly enqueue it if `isInMergeQueue=false`, and watch the
   `merge_group` run rather than admission stubs; record retry latency.
7. After the merge lands, fast-forward and prepare the clean tag worktree:
   `git fetch origin main`; `LANDED_MAIN="$(git rev-parse origin/main)"`;
   `VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' cas-cli/Cargo.toml | head -n1)"`;
   `TAG="v$VERSION"`; `TAG_WORKTREE=".cas/release-$TAG"`;
   `git worktree add "$TAG_WORKTREE" "$LANDED_MAIN"`; `mkdir -p
   "$TAG_WORKTREE/.context"`; `cp -al .context/zig "$TAG_WORKTREE/.context/zig"`.
   Do not run local preflight here:
   `release.sh --publish-tag` owns annotated-tag creation and runs
   `check-release-preflight.sh --local` before pushing the tag.
8. From that tag worktree, run this exact publisher wrapper:

   ```bash
   cd "$TAG_WORKTREE"
   test -x "$PWD/.context/zig/zig"
   export ZIG="$PWD/.context/zig/zig"
   RELEASE_ENV_FILE="${CAS_RELEASE_ENV_FILE:-$HOME/.cas/release.env}"
   test -r "$RELEASE_ENV_FILE"
   set -a
   source "$RELEASE_ENV_FILE"
   set +a
   for name in CAS_POSTHOG_API_KEY CAS_SENTRY_DSN; do
     if [[ -v "$name" ]]; then printf '%s: set\n' "$name"; else printf '%s: unset\n' "$name"; fi
   done
   # Set CAS_RELEASE_ARTIFACTS_ROOT to the configured [factory] artifacts_root.
   ARTIFACTS_ROOT="${CAS_RELEASE_ARTIFACTS_ROOT:-$HOME/.cas/artifacts}"
   EVIDENCE_DIR="$ARTIFACTS_ROOT/release/$TAG"
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

   `CAS_RELEASE_ENV_FILE` is configurable and defaults to the user-level
   `$HOME/.cas/release.env`. The credential proof prints names and set/unset state only. The inner
   `set +e` leaves an outer shell's errexit unchanged while capturing `$?` and
   writing the numeric done receipt. The log, PID, and done receipt must stay
   under `ARTIFACTS_ROOT`, outside the tag worktree. At reminder wakeups inspect
   `kill -0 "$(cat "$PID_FILE")"` and `test -s "$DONE"`; do not `wait`,
   foreground-poll, or use `pkill -f`.
9. Before announcing, require every receipt: verify the annotated tag peels to
   `LANDED_MAIN` and `git ls-remote --exit-code --refs origin "refs/tags/$TAG"`
   succeeds; save the matching `gh run list --workflow release.yml --limit 20
   --json databaseId,headSha,status,conclusion` row and successful conclusion;
   run `release-published-receipt.sh --write-draft` and
   `release-latency-receipt.sh`; use MechaCassy's default `cas-internal` channel
   name for preflight and posts, retaining `C0B44GUKDK2` only for receipt
   verification, and save four Slack POSTED entries with timestamp and
   permalink. If a live Cassy proxy lacks the new registration, use the direct
   configured mecha-cassy MCP or approved bounded one-shot route; do not retry
   `cas`/`mcp_execute` after its authenticated-session rejection. Save `cas
   update`, `cas --version`, and `cas hub` proof under `EVIDENCE_DIR`.
10. Carry the POSTED receipt into the next prep commit. Close only after the
    merge receipt and stranded-branch inspection; if sibling lanes rewrote
    delivered files, use `stranded_branch_override` with proof on main.
