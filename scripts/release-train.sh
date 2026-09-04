#!/usr/bin/env bash
# Owns one release-gate RUN: its directory, its receipts, and its process.
#
# Why this exists (cas-5212). The release train used to be driven by wrappers
# hand-written per release under ~/.cas/artifacts/release/v<version>/, and both
# the artifacts directory and the ad-hoc "find the gate" commands were keyed on
# the VERSION STRING. A version string does not identify a run: on 2026-09-04
# two supervisors gated 3.15.2 concurrently from different epic worktrees
# (.cas/epic-4f6f-merge and .cas/epic-cas-8094-merge). Both resolved the same
# directory, so gate.log and gate.done were mutually overwritable and an agent
# reported one supervisor's gate state to the other. A `pgrep -f
# 'release-gate.sh 3.15.2' | head -1` typed during that incident matched BOTH
# processes; acting on `head -1` picks by pid ordering, not by ownership.
#
# The rules this script exists to make unbreakable:
#   * the run directory is keyed by version AND worktree, never version alone;
#   * a run is located by a PID this script recorded, never by a name pattern;
#   * `--stop` signals only that recorded pid, so a sibling run survives.
#
# Usage:
#   scripts/release-train.sh <version> <epic-worktree> --gate
#   scripts/release-train.sh <version> <epic-worktree> --pipeline
#   scripts/release-train.sh <version> <epic-worktree> --status
#   scripts/release-train.sh <version> <epic-worktree> --stop
#   scripts/release-train.sh <version> <epic-worktree> --print-run-dir
#
# Environment seams (defaults are the real thing; the self-test overrides them):
#   CAS_RELEASE_ARTIFACTS_ROOT   default ~/.cas/artifacts/release
#   CAS_RELEASE_TRAIN_GATE_CMD   default <worktree>/scripts/release-gate.sh
#   CAS_RELEASE_TRAIN_PROXY_TOML default <main checkout>/.cas/proxy.toml
#   CAS_RELEASE_TRAIN_GH          default gh
#   CAS_RELEASE_TRAIN_BRANCH      default the epic worktree's current branch
#   CAS_RELEASE_TRAIN_POLL_SECS   default 45 (checks) / 60 (queue watch)
#   CAS_RELEASE_TRAIN_CHECK_TRIES default 40
#   CAS_RELEASE_TRAIN_WATCH_TRIES default 60
set -euo pipefail

usage() {
    printf 'Usage: %s <version> <epic-worktree> [--gate|--pipeline|--status|--stop|--print-run-dir]\n' "$0"
}

version="${1:-}"
worktree="${2:-}"
action="${3:---gate}"

if [[ -z "$version" || -z "$worktree" ]]; then
    usage >&2
    exit 2
fi
if [[ ! -d "$worktree" ]]; then
    printf 'error: epic worktree %s does not exist\n' "$worktree" >&2
    exit 2
fi

worktree="$(cd "$worktree" && pwd)"
worktree_name="$(basename "$worktree")"
artifacts_root="${CAS_RELEASE_ARTIFACTS_ROOT:-$HOME/.cas/artifacts/release}"
# The identity of a run: which version, from which worktree. Two supervisors
# cutting the same version from different epics get different directories.
run_dir="$artifacts_root/v$version-$worktree_name"
pid_file="$run_dir/gate.pid"

# The pid recorded for this run, if it is still alive. Liveness is asked of the
# recorded pid directly — never inferred from a process name.
live_gate_pid() {
    local pid
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    [[ -n "$pid" ]] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    printf '%s\n' "$pid"
}

write_run_env() {
    local tip
    tip="$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    cat >"$run_dir/run.env" <<EOF
version=$version
worktree=$worktree
worktree_name=$worktree_name
repository=$(git -C "$worktree" rev-parse --show-toplevel 2>/dev/null || echo unknown)
tip=$tip
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
started_by_pid=$$
EOF
}

# --------------------------------------------------------------------------
# pipeline: push -> PR -> required checks -> merge queue -> landed (cas-da81)
#
# Ported from the hand-written pipeline.sh used for v3.15.0-v3.15.2, keeping
# its semantics and encoding the mistake that cost a release its queue entry:
# a push-triggered CI run contributes rows carrying the required check NAMES
# with bucket "skipped". Treating those as satisfied enqueued the PR before the
# pull_request run existed, GitHub dropped the entry silently
# (mergeQueueEntry null), and the watch loop had to notice "no entry and no new
# queue run" to recover. Both halves are enforced below.
# --------------------------------------------------------------------------
gh_cmd() { "${CAS_RELEASE_TRAIN_GH:-gh}" "$@"; }

pipeline_log() { printf '%s %s\n' "$(date -u +%H:%M:%SZ)" "$*"; }

pipeline_finish() {
    local state="$1"
    printf '%s\n' "$state" >"$run_dir/pipeline.done"
    pipeline_log "pipeline terminal state: $state"
}

# At least one bucket==pass row for EVERY required check. A skipped row carries
# the same name and proves nothing, so it is treated as absent.
required_checks_pass() {
    local checks
    checks="$(gh_cmd pr checks "$pr_number" -R "$repo_slug" --json name,bucket 2>/dev/null || printf '[]')"
    printf '%s' "$checks" | jq -e '
        (map(select(.name == "Fast Validation" and .bucket == "pass")) | length) >= 1
        and (map(select(.name == "macOS Check" and .bucket == "pass")) | length) >= 1
    ' >/dev/null 2>&1
}

run_pipeline() {
    local gate_status
    gate_status="$(cat "$run_dir/gate.done" 2>/dev/null || true)"
    if [[ "$gate_status" != "0" ]]; then
        pipeline_log "GATE_NOT_GREEN (gate.done=${gate_status:-absent}) in $run_dir"
        pipeline_finish GATE_NOT_GREEN
        return 1
    fi

    local branch="${CAS_RELEASE_TRAIN_BRANCH:-}"
    if [[ -z "$branch" ]]; then
        branch="$(git -C "$worktree" rev-parse --abbrev-ref HEAD)"
    fi
    if [[ "$branch" == "HEAD" ]]; then
        pipeline_log "the epic worktree is detached; set CAS_RELEASE_TRAIN_BRANCH to the branch to push"
        pipeline_finish NO_BRANCH
        return 2
    fi

    local repo_slug="${CAS_RELEASE_TRAIN_REPO:-Richards-LLC/cassy}"
    local poll="${CAS_RELEASE_TRAIN_POLL_SECS:-45}"
    local check_tries="${CAS_RELEASE_TRAIN_CHECK_TRIES:-40}"
    local watch_tries="${CAS_RELEASE_TRAIN_WATCH_TRIES:-60}"

    pipeline_log "pipeline start branch=$branch tip=$(git -C "$worktree" rev-parse --short HEAD)"
    git -C "$worktree" push -q origin "HEAD:refs/heads/$branch" && pipeline_log "pushed $branch"

    local pr_number
    pr_number="$(gh_cmd pr list -R "$repo_slug" --head "$branch" --json number 2>/dev/null \
        | jq -r 'if type == "array" then (.[0].number // empty) else empty end' 2>/dev/null || true)"
    if [[ -z "$pr_number" ]]; then
        pr_number="$(gh_cmd pr create -R "$repo_slug" --base main --head "$branch" \
            --title "Release $version" --body-file "$run_dir/pr-body.md" | grep -oE '[0-9]+$' | tail -1)"
    fi
    if [[ -z "$pr_number" ]]; then
        pipeline_log "NO_PR: could not create or find a pull request for $branch"
        pipeline_finish NO_PR
        return 1
    fi
    printf '%s\n' "$pr_number" >"$run_dir/pr-number.txt"
    pipeline_log "PR #$pr_number"

    {
        printf 'Gate receipt — release-train.sh %s on %s:\n' "$version" "$(git -C "$worktree" rev-parse --short HEAD)"
        printf '```\n'
        grep -E '^(PASS|FAIL)' "$run_dir/gate.log" | cut -c1-110
        printf '```\n'
    } | gh_cmd pr comment "$pr_number" -R "$repo_slug" --body-file - >/dev/null 2>&1 \
        && pipeline_log "gate receipt commented"

    local i
    for ((i = 1; i <= check_tries; i++)); do
        if required_checks_pass; then
            pipeline_log "required checks pass"
            break
        fi
        pipeline_log "required checks not yet green (attempt $i/$check_tries)"
        sleep "$poll"
    done
    if ! required_checks_pass; then
        pipeline_log "CHECKS_NEVER_PASSED — not enqueuing; an enqueue before the pull_request run exists is dropped silently"
        pipeline_finish CHECKS_FAILED
        return 1
    fi

    local pr_id
    pr_id="$(gh_cmd pr view "$pr_number" -R "$repo_slug" --json id 2>/dev/null \
        | jq -r '.id // empty' 2>/dev/null || true)"
    local since
    since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    enqueue() {
        local out
        out="$(gh_cmd api graphql -f query='mutation($id:ID!){enqueuePullRequest(input:{pullRequestId:$id}){mergeQueueEntry{position state}}}' -F id="$pr_id" 2>&1 || true)"
        if printf '%s' "$out" | grep -q '"state"'; then
            pipeline_log "enqueued"
            return 0
        fi
        pipeline_log "enqueue failed: $(printf '%s' "$out" | tr -d '\n' | cut -c1-140)"
        return 1
    }

    enqueue || { sleep "$poll"; enqueue || true; }

    local requeues=0
    for ((i = 1; i <= watch_tries; i++)); do
        local state merge_oid queue_run entry
        state="$(gh_cmd pr view "$pr_number" -R "$repo_slug" --json state,mergeCommit 2>/dev/null \
            | jq -r '"\(.state) \(.mergeCommit.oid // "")"' 2>/dev/null || printf 'UNKNOWN ')"
        merge_oid="${state#* }"
        queue_run="$(gh_cmd run list -R "$repo_slug" --event merge_group --limit 3 --json databaseId,status,conclusion,createdAt 2>/dev/null \
            | jq -r --arg s "$since" '[.[] | select(.createdAt > $s)] | .[0] | if . == null then "none-yet" else "\(.databaseId) \(.status)/\(.conclusion // "-")" end' 2>/dev/null || printf 'none-yet')"
        entry="$(gh_cmd api graphql -f query="{repository(owner:\"${repo_slug%%/*}\",name:\"${repo_slug##*/}\"){pullRequest(number:$pr_number){mergeQueueEntry{state}}}}" 2>/dev/null | tr -d '\n' || true)"
        entry="${entry:-no-entry}"
        pipeline_log "pr: $state | queue-run: $queue_run | entry: $entry"

        case "$state" in
            MERGED*)
                git -C "$worktree" fetch -q origin main || true
                local landed
                landed="$(git -C "$worktree" rev-parse origin/main 2>/dev/null || printf '%s' "$merge_oid")"
                printf '%s\n' "$landed" >"$run_dir/landed-main.sha"
                pipeline_log "PR_MERGED landed=$landed"
                pipeline_finish MERGED
                return 0
                ;;
        esac
        case "$queue_run" in
            *completed/failure)
                pipeline_log "QUEUE_RUN_FAILED $queue_run"
                pipeline_finish QUEUE_RUN_FAILED
                return 1
                ;;
        esac
        # The dropped-entry signature: GitHub accepted the mutation, then the
        # entry vanished without ever starting a merge_group run.
        if [[ "$entry" == "no-entry" && "$queue_run" == "none-yet" && $i -gt 1 ]]; then
            requeues=$((requeues + 1))
            if [[ $requeues -le 3 ]]; then
                pipeline_log "entry dropped — re-enqueue #$requeues"
                since="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
                enqueue || true
            else
                pipeline_log "DROPPED_TOO_OFTEN"
                pipeline_finish DROPPED_TOO_OFTEN
                return 1
            fi
        fi
        sleep "$poll"
    done
    pipeline_log TIMEOUT
    pipeline_finish TIMEOUT
    return 1
}

case "$action" in
    --print-run-dir)
        printf '%s\n' "$run_dir"
        exit 0
        ;;
    --status)
        printf 'run directory: %s\n' "$run_dir"
        if [[ -f "$run_dir/run.env" ]]; then
            cat "$run_dir/run.env"
        else
            printf 'no run recorded yet\n'
        fi
        if pid="$(live_gate_pid)"; then
            printf 'gate: running (pid %s)\n' "$pid"
        elif [[ -f "$run_dir/gate.done" ]]; then
            printf 'gate: finished with status %s\n' "$(cat "$run_dir/gate.done")"
        else
            printf 'gate: not running\n'
        fi
        exit 0
        ;;
    --stop)
        if pid="$(live_gate_pid)"; then
            # Only ever the pid this run recorded. No pattern, no `head -1`.
            kill -TERM "$pid"
            printf 'signalled gate pid %s for %s\n' "$pid" "$run_dir"
            exit 0
        fi
        printf 'no live gate recorded for %s; nothing signalled\n' "$run_dir" >&2
        exit 1
        ;;
    --pipeline)
        mkdir -p "$run_dir"
        # Piped rather than process-substituted so the shell waits for tee:
        # a Monitor tailing pipeline.log must see every line the run wrote,
        # including the terminal one.
        run_pipeline 2>&1 | tee -a "$run_dir/pipeline.log"
        exit "${PIPESTATUS[0]}"
        ;;
    --gate) ;;
    *)
        usage >&2
        exit 2
        ;;
esac

mkdir -p "$run_dir"

# Refuse rather than race. The check is against the pid this run recorded, so a
# sibling supervisor's gate is invisible here — as it should be.
if pid="$(live_gate_pid)"; then
    owner_worktree="$(sed -n 's/^worktree=//p' "$run_dir/run.env" 2>/dev/null || true)"
    printf 'refusing to start: a gate for %s is already in progress for worktree %s (pid %s).\n' \
        "$version" "${owner_worktree:-$worktree}" "$pid" >&2
    printf 'Inspect it with `%s %s %s --status`, wait for it to finish, or stop it with `%s %s %s --stop` — only if that run is yours.\n' \
        "$0" "$version" "$worktree" "$0" "$version" "$worktree" >&2
    exit 3
fi

gate_cmd="${CAS_RELEASE_TRAIN_GATE_CMD:-$worktree/scripts/release-gate.sh}"
if [[ ! -x "$gate_cmd" ]]; then
    printf 'error: gate command %s is not executable\n' "$gate_cmd" >&2
    exit 2
fi

# The host-local .cas/proxy.toml leaks into hermetic proxy tests through the
# ancestor lookup (cas-4ccc), so it is moved aside for the run and restored on
# exit — including when the gate is killed.
main_checkout="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##' || true)"
proxy_toml="${CAS_RELEASE_TRAIN_PROXY_TOML:-${main_checkout:-$worktree}/.cas/proxy.toml}"
proxy_aside="$proxy_toml.gate-aside"

restore_proxy() {
    if [[ -f "$proxy_aside" ]]; then
        mv "$proxy_aside" "$proxy_toml"
        printf 'proxy.toml restored %s\n' "$(date -u +%H:%M:%SZ)"
    fi
}
trap restore_proxy EXIT

if [[ -f "$proxy_toml" ]]; then
    mv "$proxy_toml" "$proxy_aside"
    printf 'proxy.toml moved aside %s\n' "$(date -u +%H:%M:%SZ)"
fi

write_run_env
rm -f "$run_dir/gate.done"

printf 'gate start %s version=%s worktree=%s tip=%s\n' \
    "$(date -u +%H:%M:%SZ)" "$version" "$worktree" \
    "$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null || echo unknown)"
printf 'run directory: %s\n' "$run_dir"

(
    cd "$worktree"
    [[ -x "$PWD/.context/zig/zig" ]] && export ZIG="$PWD/.context/zig/zig"
    export CAS_RELEASE_GATE_HOME_DIR="${CAS_RELEASE_GATE_HOME_DIR:-/var/tmp/cas-release-gate}"
    exec "$gate_cmd" "$version"
) >"$run_dir/gate.log" 2>&1 &
gate_pid=$!
printf '%s\n' "$gate_pid" >"$pid_file"

set +e
wait "$gate_pid"
rc=$?
set -e
printf '%s\n' "$rc" >"$run_dir/gate.done"
printf 'gate rc=%s end %s\n' "$rc" "$(date -u +%H:%M:%SZ)"
exit "$rc"
