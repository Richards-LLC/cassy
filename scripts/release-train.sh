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
#   * `--stop` signals only that recorded process group, so its children die
#     and a sibling run survives.
#
# Usage:
#   scripts/release-train.sh <version> <epic-worktree> --check-lane <branch>
#   scripts/release-train.sh <version> <epic-worktree> --gate [--only <row,row>]
#   scripts/release-train.sh <version> <epic-worktree> --pipeline
#   scripts/release-train.sh <version> <epic-worktree> --publish [<landed-sha>]
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
#   CAS_RELEASE_TRAIN_PUBLISH_CMD default <tag worktree>/scripts/release.sh
#   CAS_RELEASE_ENV_FILE          default ~/.cas/release.env
set -euo pipefail

usage() {
    printf 'Usage: %s <version> <epic-worktree> [--check-lane <branch>|--gate [--only <row,row>]|--pipeline|--publish [sha]|--status|--stop|--print-run-dir]\n' "$0"
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
readonly -a gate_rows=(
    scratch-base epic-worktree-fresh epic-worktree-zig failure-log ancestor-proxy-config
    version-literals fixture-paths workspace-tests nextest doctests archive-mode
    snapshot-portability builtin-projections changelog-and-versions release-script
    procedure-guardrails working-tree
)

valid_gate_row() {
    local candidate="$1" row
    for row in "${gate_rows[@]}"; do
        [[ "$candidate" == "$row" ]] && return 0
    done
    return 1
}

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
    local env_file="${1:-$run_dir/run.env}" tip tip_sha
    tip="$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    tip_sha="$(git -C "$worktree" rev-parse HEAD 2>/dev/null || echo unknown)"
    cat >"$env_file" <<EOF
version=$version
worktree=$worktree
worktree_name=$worktree_name
repository=$(git -C "$worktree" rev-parse --show-toplevel 2>/dev/null || echo unknown)
tip=$tip
tip_sha=$tip_sha
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

check_lane() {
    local branch="$1" repo_slug sha runs row run_id jobs job status conclusion job_id
    [[ -n "$branch" ]] || {
        printf 'error: --check-lane requires a branch\n' >&2
        return 2
    }
    repo_slug="${CAS_RELEASE_TRAIN_REPO:-Richards-LLC/cassy}"
    sha="$(git -C "$worktree" rev-parse --verify --quiet "refs/heads/$branch^{commit}" 2>/dev/null \
        || git -C "$worktree" rev-parse --verify --quiet "refs/remotes/origin/$branch^{commit}" 2>/dev/null || true)"
    [[ -n "$sha" ]] || {
        printf 'lane %s: MISSING (branch tip not found locally)\n' "$branch"
        return 1
    }
    if ! runs="$(gh_cmd run list -R "$repo_slug" --workflow ci.yml --branch "$branch" \
        --event push --limit 20 --json databaseId,headBranch,headSha,status,conclusion,event,workflowName 2>&1)"; then
        printf 'lane %s at %s: API ERROR listing CI push runs: %s\n' "$branch" "$sha" "$runs"
        return 1
    fi
    if ! printf '%s' "$runs" | jq -e 'type == "array"' >/dev/null 2>&1; then
        printf 'lane %s at %s: API ERROR parsing CI push runs\n' "$branch" "$sha"
        return 1
    fi
    row="$(printf '%s' "$runs" | jq -c --arg branch "$branch" --arg sha "$sha" '
        [.[] | select(.headBranch == $branch and .headSha == $sha and .event == "push"
          and .workflowName == "CI")] | first // empty')"
    if [[ -z "$row" ]]; then
        printf 'lane %s at %s: MISSING exact-sha CI push run\n' "$branch" "$sha"
        return 1
    fi
    run_id="$(printf '%s' "$row" | jq -r '.databaseId // "unknown"')"
    if ! jobs="$(gh_cmd run view "$run_id" -R "$repo_slug" --json jobs 2>&1)"; then
        printf 'lane %s at %s: API ERROR reading CI run %s jobs: %s\n' \
            "$branch" "$sha" "$run_id" "$jobs"
        return 1
    fi
    if ! printf '%s' "$jobs" | jq -e '.jobs | type == "array"' >/dev/null 2>&1; then
        printf 'lane %s at %s: API ERROR parsing CI run %s jobs\n' "$branch" "$sha" "$run_id"
        return 1
    fi
    job="$(printf '%s' "$jobs" | jq -c '[.jobs[] | select(.name == "Scoped Validation (factory/PR)")] | first // empty')"
    if [[ -z "$job" ]]; then
        printf 'lane %s at %s: MISSING Scoped Validation (factory/PR) job in CI run %s\n' "$branch" "$sha" "$run_id"
        return 1
    fi
    status="$(printf '%s' "$job" | jq -r '.status // "unknown"')"
    conclusion="$(printf '%s' "$job" | jq -r '.conclusion // "pending"')"
    job_id="$(printf '%s' "$job" | jq -r '.databaseId // "unknown"')"
    printf 'lane %s at %s: CI run %s Scoped Validation (factory/PR) job %s status=%s conclusion=%s\n' \
        "$branch" "$sha" "$run_id" "$job_id" "$status" "$conclusion"
    if [[ "$status" != completed ]]; then
        printf 'lane %s: PENDING; refusing release-bound merge\n' "$branch"
        return 1
    fi
    if [[ "$conclusion" != success ]]; then
        printf 'lane %s: RED (%s); refusing release-bound merge\n' "$branch" "$conclusion"
        return 1
    fi
    printf 'lane %s: GREEN; eligible for release-bound merge\n' "$branch"
}

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
    local gate_status gate_sha current_sha
    gate_status="$(cat "$run_dir/gate.done" 2>/dev/null || true)"
    gate_sha="$(cat "$run_dir/gate.full.sha" 2>/dev/null || true)"
    current_sha="$(git -C "$worktree" rev-parse HEAD 2>/dev/null || true)"
    if [[ "$gate_status" != "0" || -z "$gate_sha" ]]; then
        pipeline_log "GATE_NOT_GREEN (full gate.done=${gate_status:-absent} gate.full.sha=${gate_sha:-absent}) in $run_dir"
        pipeline_finish GATE_NOT_GREEN
        return 1
    fi
    if [[ -z "$current_sha" || "$gate_sha" != "$current_sha" ]]; then
        pipeline_log "STALE_FULL_GATE (proved=$gate_sha current=${current_sha:-absent}); run the full gate on the current tree"
        pipeline_finish STALE_FULL_GATE
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

# --------------------------------------------------------------------------
# publish: tag worktree at the landed sha -> release.sh --publish-tag (cas-c1cd)
#
# Ported from publish-wrapper.sh. Every refusal fires BEFORE a tag worktree
# exists or a publisher starts, because publishing the wrong tree is not
# undone by retrying. The receipts live in the run directory rather than a
# version-keyed path, and the publisher is waited on by the PID recorded for
# it — the same discipline the gate uses.
# --------------------------------------------------------------------------
run_publish() {
    local landed="${1:-}"
    if [[ -z "$landed" ]]; then
        landed="$(cat "$run_dir/landed-main.sha" 2>/dev/null | tr -d '[:space:]' || true)"
    fi
    if [[ -z "$landed" ]]; then
        printf 'error: no landed sha given and %s/landed-main.sha is absent; run --pipeline first\n' \
            "$run_dir" >&2
        return 2
    fi

    git -C "$worktree" fetch -q origin main 2>/dev/null || true
    local origin_main
    origin_main="$(git -C "$worktree" rev-parse origin/main 2>/dev/null || true)"
    if [[ "$origin_main" != "$landed" ]]; then
        printf 'error: origin/main is %s, not the landed sha %s; refusing to publish\n' \
            "${origin_main:-unknown}" "$landed" >&2
        return 3
    fi

    # Read the version out of the landed commit itself rather than the working
    # tree, so a dirty epic worktree cannot vouch for what is being tagged.
    local landed_version
    landed_version="$(git -C "$worktree" show "$landed:cas-cli/Cargo.toml" 2>/dev/null \
        | sed -n 's/^version = "\([^"]*\)"/\1/p' | head -n1)"
    if [[ "$landed_version" != "$version" ]]; then
        printf 'error: the landed tree at %s declares version %s, but this run is publishing %s; refusing\n' \
            "$landed" "${landed_version:-unknown}" "$version" >&2
        return 4
    fi

    local tag="v$version"
    local main_checkout_dir
    main_checkout_dir="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##')"
    main_checkout_dir="${main_checkout_dir:-$worktree}"
    local tag_worktree="$main_checkout_dir/.cas/release-$tag"

    if [[ ! -d "$tag_worktree" ]]; then
        git -C "$worktree" worktree add --detach "$tag_worktree" "$landed" >/dev/null 2>&1 || {
            printf 'error: could not create the tag worktree at %s\n' "$tag_worktree" >&2
            return 5
        }
    fi
    if [[ "$(git -C "$tag_worktree" rev-parse HEAD 2>/dev/null)" != "$landed" ]]; then
        printf 'error: tag worktree %s is not at %s; refusing to publish\n' "$tag_worktree" "$landed" >&2
        return 6
    fi

    # The zig toolchain is hardlinked rather than copied: same bytes, no second
    # multi-hundred-megabyte tree per release.
    if [[ -d "$main_checkout_dir/.context/zig" && ! -e "$tag_worktree/.context/zig" ]]; then
        mkdir -p "$tag_worktree/.context"
        cp -al "$main_checkout_dir/.context/zig" "$tag_worktree/.context/zig" 2>/dev/null || true
    fi
    [[ -x "$tag_worktree/.context/zig/zig" ]] && export ZIG="$tag_worktree/.context/zig/zig"

    local env_file="${CAS_RELEASE_ENV_FILE:-$HOME/.cas/release.env}"
    if [[ -f "$env_file" ]]; then
        set -a
        # shellcheck disable=SC1090
        . "$env_file"
        set +a
        # Names and set/unset state only: a release receipt must never carry a
        # credential value.
        printf 'release env names: %s\n' \
            "$(grep -oE '^(export )?[A-Z_]+=' "$env_file" | sed 's/=$//; s/^export //' | tr '\n' ' ')"
    else
        printf 'release env file %s absent; publishing with the ambient environment\n' "$env_file"
    fi

    local publish_cmd="${CAS_RELEASE_TRAIN_PUBLISH_CMD:-$tag_worktree/scripts/release.sh}"
    if [[ ! -x "$publish_cmd" ]]; then
        printf 'error: publish command %s is not executable\n' "$publish_cmd" >&2
        return 7
    fi

    printf 'publisher start %s sha=%s tag=%s worktree=%s\n' \
        "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$landed" "$tag" "$tag_worktree"
    printf 'run directory: %s\n' "$run_dir"

    ( cd "$tag_worktree" && exec "$publish_cmd" --publish-tag ) \
        >"$run_dir/release.log" 2>&1 &
    local publish_pid=$!
    printf '%s\n' "$publish_pid" >"$run_dir/release.pid"

    set +e
    wait "$publish_pid"
    local rc=$?
    set -e
    printf '%s\n' "$rc" >"$run_dir/release.done"
    if [[ "$rc" -eq 0 ]]; then
        date -u +%s >"$run_dir/release.published.epoch"
    fi
    printf 'publisher done status=%s at %s\n' "$rc" "$(date -u +%H:%M:%SZ)"
    return "$rc"
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
        if [[ -s "$run_dir/gate.log" ]]; then
            failed_rows="$(sed -n 's/^FAIL \([^ ]*\).*/\1/p' "$run_dir/gate.log" | paste -sd, -)"
            tip="$(sed -n 's/^tip=//p' "$run_dir/run.env" 2>/dev/null || printf unknown)"
            printf 'epic-note template: tip=%s rows_failed=%s cause_class=<product|fixture|environment|procedure> blocking_step=<step>\n' \
                "${tip:-unknown}" "${failed_rows:-none}"
        fi
        if [[ -s "$run_dir/gate.green.epoch" && -s "$run_dir/release.published.epoch" ]]; then
            green_epoch="$(cat "$run_dir/gate.green.epoch")"
            published_epoch="$(cat "$run_dir/release.published.epoch")"
            printf 'green-to-published latency: %ss\n' "$((published_epoch - green_epoch))"
        fi
        exit 0
        ;;
    --stop)
        if pid="$(live_gate_pid)"; then
            # The detached gate owns a fresh session/process group whose id is
            # the recorded pid. Signal that group so nextest/git descendants do
            # not survive their parent gate.
            kill -TERM -- "-$pid"
            printf 'signalled gate process group %s for %s\n' "$pid" "$run_dir"
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
    --publish)
        mkdir -p "$run_dir"
        run_publish "${4:-}" 2>&1 | tee -a "$run_dir/publish.log"
        exit "${PIPESTATUS[0]}"
        ;;
    --check-lane)
        check_lane "${4:-}"
        exit $?
        ;;
    --gate)
        only_rows=''
        if [[ "${4:-}" == '--only' ]]; then
            only_rows="${5:-}"
            [[ -n "$only_rows" && "$#" -eq 5 ]] || {
                printf 'error: --only requires a non-empty comma-separated row list\n' >&2
                exit 2
            }
            IFS=',' read -r -a requested_rows <<<"$only_rows"
            for requested in "${requested_rows[@]}"; do
                if [[ -z "$requested" ]] || ! valid_gate_row "$requested"; then
                    printf 'error: unknown --only release-gate row %s\n' "${requested:-<empty>}" >&2
                    exit 2
                fi
            done
        elif [[ "$#" -ne 3 ]]; then
            usage >&2
            exit 2
        fi
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

mkdir -p "$run_dir"

# A full gate owns the authorization receipts consumed by --pipeline. Targeted
# --only reruns are diagnostics: keep them in an append-only subdirectory so a
# partial success cannot overwrite or manufacture full-gate authorization.
if [[ -n "${only_rows:-}" ]]; then
    diagnostic_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
    receipt_dir="$run_dir/diagnostics/$diagnostic_id"
    mkdir -p "$receipt_dir"
    gate_done_file="$receipt_dir/gate.done"
    gate_green_file=''
    gate_sha_file=''
    gate_log_file="$receipt_dir/gate.log"
    run_env_file="$receipt_dir/run.env"
else
    receipt_dir="$run_dir"
    gate_done_file="$run_dir/gate.done"
    gate_green_file="$run_dir/gate.green.epoch"
    gate_sha_file="$run_dir/gate.full.sha"
    gate_log_file="$run_dir/gate.log"
    run_env_file="$run_dir/run.env"
fi

# Builtin reference history is a content ledger, so it is regenerated only
# after every merge and --learn edit is complete. Refuse before detaching a
# slow gate when the generated bytes are not committed.
reference_history_script="$worktree/scripts/gen-builtin-reference-history.sh"
if [[ -x "$reference_history_script" ]]; then
    (cd "$worktree" && "$reference_history_script")
    if ! git -C "$worktree" diff --quiet -- cas-cli/src/builtins/reference-history.json; then
        printf 'error: builtin reference history changed; commit the ledger before starting the detached gate\n' >&2
        git -C "$worktree" diff --stat -- cas-cli/src/builtins/reference-history.json >&2
        exit 4
    fi
fi

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

write_run_env "$run_env_file"
if [[ -z "${only_rows:-}" ]]; then
    rm -f "$gate_done_file" "$gate_green_file" "$gate_sha_file"
fi

printf 'gate start %s version=%s worktree=%s tip=%s\n' \
    "$(date -u +%H:%M:%SZ)" "$version" "$worktree" \
    "$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null || echo unknown)"
printf 'run directory: %s\n' "$run_dir"
if [[ -n "${only_rows:-}" ]]; then
    printf 'diagnostic receipt directory: %s\n' "$receipt_dir"
fi

export CAS_RELEASE_GATE_HOME_DIR="${CAS_RELEASE_GATE_HOME_DIR:-/var/tmp/cas-release-gate}"
export CAS_RELEASE_GATE_ARCHIVE_SIZE_FILE="$receipt_dir/archive-size-bytes"
gate_args=("$version")
if [[ -n "${only_rows:-}" ]]; then
    gate_args+=(--only "$only_rows")
fi
nohup setsid bash -c '
    worktree=$1; done_file=$2; green_file=$3; sha_file=$4; expected_sha=$5; mode=$6; shift 6
    cd "$worktree" || exit 125
    [[ -x "$PWD/.context/zig/zig" ]] && export ZIG="$PWD/.context/zig/zig"
    set +e
    "$@"
    rc=$?
    if [[ "$rc" -eq 0 && "$mode" == full ]]; then
        current_sha="$(git rev-parse HEAD 2>/dev/null || true)"
        if [[ "$current_sha" != "$expected_sha" ]]; then
            printf "full gate tree changed while running: expected=%s current=%s\n" \
                "$expected_sha" "${current_sha:-absent}"
            rc=1
        else
            printf "%s\n" "$expected_sha" >"$sha_file"
            date -u +%s >"$green_file"
        fi
    fi
    printf "%s\n" "$rc" >"$done_file"
    exit "$rc"
' bash "$worktree" "$gate_done_file" "$gate_green_file" "$gate_sha_file" \
    "$(git -C "$worktree" rev-parse HEAD)" "$([[ -n "${only_rows:-}" ]] && printf diagnostic || printf full)" \
    "$gate_cmd" "${gate_args[@]}" >"$gate_log_file" 2>&1 </dev/null &
gate_pid=$!
printf '%s\n' "$gate_pid" >"$pid_file"
printf 'gate detached pid=%s; use coordination remind, then --status (never a shell watcher)\n' "$gate_pid"
exit 0
