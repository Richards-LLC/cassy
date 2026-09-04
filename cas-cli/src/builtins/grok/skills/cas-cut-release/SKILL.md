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
   `cas__memory action=remember entry_type=learning tags=release`.
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
4. Before preparing the source commit, confirm no other open or queued PR
   bumps a release version (`gh pr list --state open --search release`, the
   merge-queue GraphQL query, `git ls-remote --tags origin`); a competing release
   from a second supervisor session must land first and this train takes the
   next patch number. Then prepare one source commit: run `scripts/bump-release-version.sh <version>`,
   `cargo update --workspace --offline`, update CHANGELOG, the runtime-release
   draft, and the previous POSTED receipt; then run
   `scripts/release-train.sh <version> <epic-worktree> --gate` and require every
   row PASS. Never hand-write a wrapper with a version-keyed artifacts path:
   `release-train.sh` owns a per-run directory keyed by version AND worktree
   (`v<version>-<worktree-basename>`), so two supervisors cutting the same
   version from different epics cannot overwrite each other's `gate.log` or
   `gate.done`. It records the gate's PID and refuses to start a second gate
   for that run; inspect with `--status` and stop with `--stop`, which signals
   only the recorded PID. Locate a run by its recorded PID, never by a process
   name pattern: `pgrep -f 'release-gate.sh <version>'` matches every
   concurrent supervisor's gate, and acting on `head -1` picks by pid order
   rather than by ownership (cas-5212).
5. For archive mode, the gate already puts the archive and TMPDIR under
   `/var/tmp/cas-release-gate` — a large real-disk scratch base with no `.cas`
   ancestor, never `/tmp` tmpfs. Set `CAS_RELEASE_GATE_HOME_DIR` only to move
   that base; the receipt names the base and where it came from, and the gate
   still refuses any base with a `.cas` ancestor. Build a remap
   with root `Cargo.toml` and every package path from `git ls-files
   '*/Cargo.toml'`; run outside the checkout with symlinks for
   cargo/rustc/git/sh/bash/jq/python3, no `rg`, and `--workspace-remap`.
   Exclude `component_output_test` only when its source snapshots need checkout.
6. Push the source branch and drive it to a landed merge with
   `scripts/release-train.sh <version> <epic-worktree> --pipeline`, which
   reuses the gate's run directory and writes `pr-number.txt`,
   `landed-main.sha` and a terminal `pipeline.done`
   (MERGED / CHECKS_FAILED / QUEUE_RUN_FAILED / DROPPED_TOO_OFTEN / TIMEOUT).
   It enqueues only after the pull_request run shows a `bucket == pass` row for
   BOTH Fast Validation and macOS Check: a push-triggered run contributes rows
   with the same names and `bucket == skipped`, and enqueuing on those gets the
   merge-queue entry dropped silently (`mergeQueueEntry` null), which is what
   cost v3.15.2 its first queue attempt. It also watches for that dropped
   signature — no entry and no new `merge_group` run — and re-enqueues up to
   three times before giving up (cas-da81).
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
   # Per-run, not per-version: take the same directory the gate used.
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
   update`, `cas --version`, and `cas hub` proof under `EVIDENCE_DIR`, and
   require the host `cas update -y --json` receipt to carry
   `refresh_binary_version` equal to the released version: a different value
   means the refresh ran with the pre-update binary and the host has not
   converged (cas-91ba).
10. Carry the POSTED receipt into the next prep commit. Close only after the
    merge receipt and stranded-branch inspection; if sibling lanes rewrote
    delivered files, use `stranded_branch_override` with proof on main.
