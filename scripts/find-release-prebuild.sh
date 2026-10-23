#!/usr/bin/env bash
# Locate prebuilt release artifacts for an exact commit (cas-3b7c0).
#
# Prints `found=`, `run-id=` and `reason=` for the release workflow.
#
# GITHUB_REPOSITORY defaults to the gh default repository, then the origin
# remote, for a run by hand; when neither resolves the script exits 2 with an
# error and prints no found= line (cas-0906).
#
# FAIL-SAFE, NOT FAIL-CLOSED. Every terminal failure mode — no prebuild run, a
# partial run, expired artifacts, an API outage — reports found=false, which
# routes the release back to the pre-existing cold build path. If the matching
# prebuild is queued or running, wait for it for a bounded period first so a
# prompt tag does not silently defeat the fast path. The prebuild is an
# accelerator; it can never be the reason a release cannot ship.
#
# The only accepted match is a *successful* Release Prebuild run whose head
# commit is the commit being published or has exactly its tree (cas-28eb), so
# adopted bytes always come from the tagged tree. The release builds run on
# the merge_group event: the queue builds one entry at a time and lands its
# merge commit as main, so the queue run's head is the main commit itself.
# RELEASE_PREBUILD_EVENT names the event searched for same-tree runs.
set -euo pipefail

sha="${1:-${GITHUB_SHA:-}}"
repo="${GITHUB_REPOSITORY:-}"
gh_bin="${GH_BIN:-gh}"
workflow="${RELEASE_PREBUILD_WORKFLOW:-release-prebuild.yml}"
event="${RELEASE_PREBUILD_EVENT:-merge_group}"
wait_seconds="${RELEASE_PREBUILD_WAIT_SECONDS:-900}"
poll_seconds="${RELEASE_PREBUILD_POLL_SECONDS:-15}"
api_timeout_seconds="${RELEASE_PREBUILD_API_TIMEOUT_SECONDS:-30}"
required_artifacts=("cas-x86_64-unknown-linux-gnu" "cas-aarch64-apple-darwin")

# Keep operator/test overrides bounded to non-negative integers. Invalid values
# must not turn a release lookup into an unbounded wait or a shell arithmetic
# error; the production defaults are the safe fallback.
if ! [[ "$wait_seconds" =~ ^[0-9]+$ ]]; then
    wait_seconds=900
fi
if ! [[ "$poll_seconds" =~ ^[0-9]+$ ]]; then
    poll_seconds=15
fi
if ! [[ "$api_timeout_seconds" =~ ^[1-9][0-9]*$ ]] \
    || (( ${#api_timeout_seconds} > 3 )) \
    || (( api_timeout_seconds > 300 )); then
    api_timeout_seconds=30
fi

decline() {
    printf '::warning::Release Prebuild unavailable for %s; falling back to the cold release build: %s\n' \
        "$sha" "$1" >&2
    printf 'found=false\n'
    printf 'run-id=\n'
    printf 'reason=%s\n' "$1"
    exit 0
}

if [[ -z "$sha" ]]; then
    decline "no commit SHA to look up"
fi
# owner/repo, or nothing when the text is not exactly one.
repo_slug() {
    local slug="$1"
    if [[ "$slug" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then
        printf '%s\n' "$slug"
    fi
}

# owner/repo from a GitHub remote URL (https, ssh or scp form), or nothing.
github_remote_slug() {
    local url="$1" path=""
    case "$url" in
        https://github.com/*) path="${url#https://github.com/}" ;;
        http://github.com/*) path="${url#http://github.com/}" ;;
        ssh://git@github.com/*) path="${url#ssh://git@github.com/}" ;;
        git@github.com:*) path="${url#git@github.com:}" ;;
        *) return 0 ;;
    esac
    path="${path%/}"
    path="${path%.git}"
    repo_slug "$path"
}

# A hand run has no GITHUB_REPOSITORY (cas-0906). Reporting found=false for
# that would look exactly like a missing prebuild, which is the answer an
# operator checks before pushing a release tag. So resolve the repository the
# way an operator would: the gh default repository first, then the origin
# remote. Never another remote: this checkout's `upstream` is a different
# project. If neither names exactly one GitHub repository, fail loudly; CI
# always sets GITHUB_REPOSITORY and never reaches this.
resolve_repo() {
    local slug url
    slug="$(repo_slug "$("$gh_bin" repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null || true)")"
    if [[ -n "$slug" ]]; then
        printf 'GITHUB_REPOSITORY is unset; using %s (gh repo view)\n' "$slug" >&2
        printf '%s\n' "$slug"
        return 0
    fi
    url="$(git remote get-url origin 2>/dev/null || true)"
    slug="$(github_remote_slug "$url")"
    if [[ -n "$slug" ]]; then
        printf 'GITHUB_REPOSITORY is unset; using %s (origin remote)\n' "$slug" >&2
        printf '%s\n' "$slug"
        return 0
    fi
    return 1
}

if [[ -z "$repo" ]]; then
    if ! repo="$(resolve_repo)"; then
        printf 'error: GITHUB_REPOSITORY is unset and the repository could not be resolved.\n' >&2
        printf 'error: neither "gh repo view" nor the origin remote names one GitHub repository.\n' >&2
        printf 'error: set GITHUB_REPOSITORY=owner/repo and run again; no prebuild lookup was made.\n' >&2
        exit 2
    fi
fi

api_failure_file="$(mktemp)"
trap 'rm -f "$api_failure_file"' EXIT

# Bound each API request independently. While polling, also cap the request to
# the seconds remaining in the existing wait deadline. The Ubuntu release
# runner provides coreutils `timeout`; gh errors stay private because they can
# include response details or credentials.
api_request() {
    local deadline="${1:-}" timeout_seconds="$api_timeout_seconds" remaining status http_status failure_reason
    shift || return 2
    : >"$api_failure_file"
    if [[ -n "$deadline" ]]; then
        remaining=$((deadline - SECONDS))
        if (( remaining <= 0 )); then
            printf 'timeout (exit 124)\n' >"$api_failure_file"
            return 124
        fi
        if (( remaining < timeout_seconds )); then
            timeout_seconds="$remaining"
        fi
    fi
    if timeout --signal=TERM -k 5s "${timeout_seconds}s" "$gh_bin" api "$@" 2>"$api_failure_file"; then
        : >"$api_failure_file"
        return 0
    else
        status=$?
    fi

    if (( status == 124 || status == 137 )); then
        failure_reason="timeout (exit $status)"
    else
        http_status="$(grep -oE 'HTTP [0-9]{3}' "$api_failure_file" | head -n1 || true)"
        if [[ -n "$http_status" ]]; then
            failure_reason="$http_status"
        else
            failure_reason="exit code $status"
        fi
    fi
    printf '%s\n' "$failure_reason" >"$api_failure_file"
    return "$status"
}

# Runs for this exact commit, plus recent $event runs whose head commit has its
# tree. A failed tree or recent-run lookup leaves only the exact-commit runs.
target_tree="$(api_request "" "repos/$repo/git/commits/$sha" --jq .tree.sha || true)"
fetch_runs() {
    local deadline="${1:-}" exact recent
    exact="$(api_request "$deadline" \
        "repos/$repo/actions/workflows/$workflow/runs?head_sha=$sha&per_page=20")" || return 1
    if [[ "$target_tree" =~ ^[0-9a-f]{40}$ ]] \
        && recent="$(api_request "$deadline" \
            "repos/$repo/actions/workflows/$workflow/runs?event=$event&per_page=30")" \
        && jq -s --arg tree "$target_tree" \
            '{workflow_runs: ([.[0].workflow_runs[]?]
              + [.[1].workflow_runs[]? | select(.head_commit.tree_id == $tree)] | unique_by(.id))}' \
            <<<"$exact$recent" 2>/dev/null; then
        return 0
    fi
    printf '%s\n' "$exact"
}

has_pending_run() {
    jq -e '[.workflow_runs[]? | select(.status == "queued" or .status == "in_progress")] | length > 0' \
        <<<"$1" >/dev/null 2>&1
}

complete_run_id=""
artifacts_query_failed=false
find_complete_run() {
    local runs_json="$1" deadline="${2:-}" run_id artifacts_json complete name live
    local artifact_query_ok artifact_attempt artifact_retry_limit
    local remaining sleep_for failure_reason permanent_failure=""
    complete_run_id=""
    artifacts_query_failed=false
    mapfile -t run_ids < <(jq -r \
        '.workflow_runs[]? | select(.status == "completed" and .conclusion == "success") | .id // empty' \
        <<<"$runs_json" 2>/dev/null || true)

    for run_id in "${run_ids[@]}"; do
        artifacts_json=""
        artifact_query_ok=false
        artifact_attempt=0
        # Allow one transient retry without spending the whole poll's deadline
        # on a single run when another completed run may have usable artifacts.
        artifact_retry_limit=2
        while :; do
            artifact_attempt=$((artifact_attempt + 1))
            if artifacts_json="$(api_request "$deadline" \
                "repos/$repo/actions/runs/$run_id/artifacts?per_page=100")"; then
                artifact_query_ok=true
                break
            fi
            failure_reason="$(<"$api_failure_file")"
            failure_reason="${failure_reason:-exit code 1}"
            # Definite HTTP client failures cannot recover by waiting. Still
            # inspect other completed runs before declining the prebuild.
            if [[ "$failure_reason" == HTTP\ 4[0-9][0-9] ]]; then
                permanent_failure="run $run_id ($failure_reason)"
                break
            fi
            # The initial lookup tries once. During the wait, retry once here,
            # then check other runs; later polls can retry this run again.
            if [[ -z "$deadline" ]]; then
                break
            fi
            remaining=$((deadline - SECONDS))
            if (( remaining <= 0 )); then
                printf 'warning: Release Prebuild artifact lookup attempt %d for run %s failed (%s) at the deadline\n' \
                    "$artifact_attempt" "$run_id" "$failure_reason" >&2
                break
            fi
            if (( artifact_attempt >= artifact_retry_limit )); then
                printf 'warning: Release Prebuild artifact lookup attempt %d for run %s failed (%s); checking remaining runs this poll\n' \
                    "$artifact_attempt" "$run_id" "$failure_reason" >&2
                break
            fi
            printf 'warning: Release Prebuild artifact lookup attempt %d for run %s failed (%s); retrying within the remaining deadline\n' \
                "$artifact_attempt" "$run_id" "$failure_reason" >&2
            sleep_for=1
            if (( sleep_for > remaining )); then
                sleep_for="$remaining"
            fi
            if (( sleep_for > 0 )); then
                sleep "$sleep_for"
            fi
            (( SECONDS < deadline )) || break
        done
        if [[ "$artifact_query_ok" != true ]]; then
            artifacts_query_failed=true
            continue
        fi
        complete=true
        for name in "${required_artifacts[@]}"; do
            if ! live="$(jq -r --arg name "$name" \
                '[.artifacts[]? | select(.name == $name and .expired == false)] | length' \
                <<<"$artifacts_json" 2>/dev/null)"; then
                live=0
            fi
            if [[ "$live" != "1" ]]; then
                complete=false
                break
            fi
        done
        if [[ "$complete" == true ]]; then
            complete_run_id="$run_id"
            return 0
        fi
    done
    if [[ -n "$permanent_failure" ]]; then
        decline "prebuild artifacts query permanent failure for $sha: $permanent_failure"
    fi
    return 1
}

runs_json=""
if ! runs_json="$(fetch_runs)"; then
    failure_reason="$(<"$api_failure_file")"
    decline "prebuild run lookup failed for $sha (${failure_reason:-exit code 1})"
fi

if find_complete_run "$runs_json"; then
    printf 'found=true\n'
    printf 'run-id=%s\n' "$complete_run_id"
    printf 'reason=prebuild run %s carries every release asset for %s\n' "$complete_run_id" "$sha"
    exit 0
fi

if has_pending_run "$runs_json" || [[ "$artifacts_query_failed" == true ]]; then
    deadline=$((SECONDS + wait_seconds))
    failed_polls=0
    run_query_failed=false
    retry_backoff=1
    retry_backoff_cap=30
    next_poll_delay="$poll_seconds"
    while (( SECONDS < deadline )); do
        remaining=$((deadline - SECONDS))
        sleep_for="$next_poll_delay"
        if (( sleep_for > remaining )); then
            sleep_for="$remaining"
        fi
        if (( sleep_for > 0 )); then
            sleep "$sleep_for"
        fi
        (( SECONDS < deadline )) || break

        polled_runs_json=""
        artifacts_query_failed=false
        run_query_failed=false
        if ! polled_runs_json="$(fetch_runs "$deadline")"; then
            run_query_failed=true
            failed_polls=$((failed_polls + 1))
            remaining=$((deadline - SECONDS))
            failure_reason="$(<"$api_failure_file")"
            failure_reason="${failure_reason:-exit code 1}"
            if (( remaining > 0 )); then
                printf 'warning: Release Prebuild lookup poll attempt %d failed (%s); retrying within the remaining deadline\n' \
                    "$failed_polls" "$failure_reason" >&2
                next_poll_delay="$retry_backoff"
                if (( retry_backoff < retry_backoff_cap )); then
                    retry_backoff=$((retry_backoff * 2))
                    if (( retry_backoff > retry_backoff_cap )); then
                        retry_backoff="$retry_backoff_cap"
                    fi
                fi
                continue
            fi
            printf 'warning: Release Prebuild lookup poll attempt %d failed (%s) at the deadline\n' \
                "$failed_polls" "$failure_reason" >&2
            break
        fi
        runs_json="$polled_runs_json"
        retry_backoff=1
        next_poll_delay="$poll_seconds"
        if find_complete_run "$runs_json" "$deadline"; then
            printf 'found=true\n'
            printf 'run-id=%s\n' "$complete_run_id"
            printf 'reason=prebuild run %s became available during the bounded wait for %s\n' \
                "$complete_run_id" "$sha"
            exit 0
        fi
        if ! has_pending_run "$runs_json" && [[ "$artifacts_query_failed" != true ]]; then
            break
        fi
    done

    if [[ "$run_query_failed" == true ]]; then
        decline "prebuild run lookup failed for $sha after ${wait_seconds}s ($failure_reason)"
    elif [[ "$artifacts_query_failed" == true ]]; then
        decline "prebuild artifacts query failed for $sha after ${wait_seconds}s"
    elif has_pending_run "$runs_json"; then
        decline "prebuild still queued or in progress after ${wait_seconds}s"
    fi
fi

decline "prebuild runs for $sha have no live copy of every release asset"
