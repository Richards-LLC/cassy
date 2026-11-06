#!/usr/bin/env bash
# Git pre-push gate for factory worker branches (cas-043b).
#
# Cassy installs a pre-push hook in worker worktrees that delegates here. A
# push of refs/heads/factory/* must carry a `release-gate.sh --fast-rows` PASS
# for the pushed tip as early local feedback; CI owns the authoritative
# proof and park ignores this receipt. An existing same-tip receipt makes a repeat
# push free. Other refs (epic branches, main) pass through.
#
# Override, logged to <git-common-dir>/cas/fast-rows/overrides.log:
#   CAS_SKIP_FAST_ROWS='<reason>' git push ...
#
# Git calls this with <remote> <url> and feeds one line per ref on stdin:
#   <local ref> <local sha> <remote ref> <remote sha>
set -uo pipefail

zero='0000000000000000000000000000000000000000'
common="$(git rev-parse --path-format=absolute --git-common-dir)" || exit 1
# Git exports GIT_DIR, GIT_INDEX_FILE and friends to hooks. The gate runs
# script tests that create fixture repositories; with those variables set, a
# fixture `git init` reinitialised the shared repository as bare (cas-f95b).
# The hook runs at the worktree root, so discovery finds the same repository.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars)
receipts="$common/cas/fast-rows"
status=0

while read -r _local_ref local_sha remote_ref remote_sha; do
    [[ "$remote_ref" == refs/heads/factory/* && "$local_sha" != "$zero" ]] || continue
    if head -n1 "$receipts/$local_sha.pass" 2>/dev/null | grep -q "^fast-rows: PASS $local_sha "; then
        continue
    fi
    if [[ -n "${CAS_SKIP_FAST_ROWS:-}" ]]; then
        mkdir -p "$receipts"
        printf '%s %s %s skipped fast rows: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$local_sha" \
            "${remote_ref#refs/heads/}" "$CAS_SKIP_FAST_ROWS" >>"$receipts/overrides.log"
        printf 'pre-push: fast rows SKIPPED for %s (%s); logged to %s\n' "${local_sha:0:12}" \
            "$CAS_SKIP_FAST_ROWS" "$receipts/overrides.log" >&2
        continue
    fi
    if [[ "$local_sha" != "$(git rev-parse HEAD)" || -n "$(git status --porcelain --untracked-files=no)" ]]; then
        printf 'pre-push: refused: %s has no fast-rows PASS and is not the clean checked-out HEAD.\n' \
            "${local_sha:0:12}" >&2
        printf 'FIX: check out %s with no uncommitted changes and push again (the hook runs the fast rows), or run ./scripts/release-gate.sh --fast-rows there first\n' \
            "${local_sha:0:12}" >&2
        status=1
        continue
    fi
    # Compare a first push with its task target, not unrelated changes on main.
    # Existing branches still compare with the tip this push replaces.
    base="$remote_sha"
    if [[ "$base" == "$zero" ]]; then
        base="${ZERO_BASE_REF:-}"
        if [[ -z "$base" ]]; then
            target="$(python3 - "$common" "$(git symbolic-ref --quiet --short HEAD 2>/dev/null)" <<'PY'
import json
import os
from pathlib import Path
import sqlite3
import sys

root = Path(os.environ.get("CAS_ROOT") or Path(sys.argv[1]).parent / ".cas").expanduser().resolve()
try:
    with sqlite3.connect((root / "cas.db").as_uri() + "?mode=ro", uri=True) as db:
        targets = set()
        for (raw,) in db.execute("SELECT deliverables FROM tasks WHERE status='in_progress'"):
            data = json.loads(raw or "{}")
            if (data.get("factory_branch_start") or {}).get("branch") == sys.argv[2]:
                target = (data.get("work_target") or {}).get("target_branch")
                if target:
                    targets.add(target)
        if len(targets) == 1:
            print(targets.pop())
except (OSError, sqlite3.Error, ValueError):
    pass  # No readable task context: retain the default-branch fallback.
PY
)" || target=''
            if [[ -n "$target" ]]; then
                for ref in "origin/$target" "$target"; do
                    if git rev-parse --verify --quiet --end-of-options "$ref^{commit}" >/dev/null; then
                        base="$ref"
                        break
                    fi
                done
            fi
        fi
        base="${base:-origin/main}"
    fi
    printf 'pre-push: running fast rows on %s (base %s)\n' "${local_sha:0:12}" "${base:0:12}" >&2
    base="$(git merge-base "$base" "$local_sha" 2>/dev/null || printf '%s' "$base")"
    # The gate's guardian removes its scratch tree on exit. Keep each push
    # attempt's complete diagnostics outside that tree, even if the gate exits
    # before it can write a receipt. A later attempt must not overwrite it.
    if ! mkdir -p "$receipts" || ! attempt_logs="$(mktemp -d "$receipts/$local_sha.logs.XXXXXX")"; then
        printf 'pre-push: refused: cannot create durable fast-row logs for %s\n' "${local_sha:0:12}" >&2
        status=1
        continue
    fi
    CAS_RELEASE_GATE_LOG_DIR="$attempt_logs" bash scripts/release-gate.sh --fast-rows --base "$base" \
        </dev/null >"$attempt_logs/gate.log" 2>&1
    if ! head -n1 "$receipts/$local_sha.pass" 2>/dev/null | grep -q "^fast-rows: PASS $local_sha "; then
        printf 'pre-push: fast-row logs: %s\n' "$attempt_logs" >&2
        grep -E '^(FAIL |FIX |RELEASE GATE FAILED)' "$attempt_logs/gate.log" >&2 \
            || tail -n 20 "$attempt_logs/gate.log" >&2
        printf 'pre-push: refused: fast rows failed on %s. Apply each FIX, commit, push again. Override (logged): CAS_SKIP_FAST_ROWS=<reason> git push\n' \
            "${local_sha:0:12}" >&2
        status=1
    else
        rm -rf -- "$attempt_logs"
    fi
done
exit "$status"
