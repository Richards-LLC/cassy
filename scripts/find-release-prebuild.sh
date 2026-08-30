#!/usr/bin/env bash
# Locate prebuilt release artifacts for an exact commit (cas-3b7c0).
#
# Prints `found=`, `run-id=` and `reason=` for the tag-time release workflow.
#
# FAIL-SAFE, NOT FAIL-CLOSED. Every terminal failure mode — no prebuild run, a
# partial run, expired artifacts, an API outage — reports found=false, which
# routes the release back to the pre-existing cold build path. If the matching
# prebuild is queued or running, wait for it for a bounded period first so a
# prompt tag does not silently defeat the fast path. The prebuild is an
# accelerator; it can never be the reason a release cannot ship.
#
# The only accepted match is a *successful* Release Prebuild run whose
# head_sha is exactly the commit being published, so adopted bytes always come
# from the tagged tree.
set -euo pipefail

sha="${1:-${GITHUB_SHA:-}}"
repo="${GITHUB_REPOSITORY:-}"
gh_bin="${GH_BIN:-gh}"
workflow="${RELEASE_PREBUILD_WORKFLOW:-release-prebuild.yml}"
wait_seconds="${RELEASE_PREBUILD_WAIT_SECONDS:-900}"
poll_seconds="${RELEASE_PREBUILD_POLL_SECONDS:-15}"
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
if [[ -z "$repo" ]]; then
    decline "GITHUB_REPOSITORY is unset"
fi

fetch_runs() {
    "$gh_bin" api \
        "repos/$repo/actions/workflows/$workflow/runs?head_sha=$sha&per_page=20" 2>/dev/null
}

has_pending_run() {
    jq -e '[.workflow_runs[]? | select(.status == "queued" or .status == "in_progress")] | length > 0' \
        <<<"$1" >/dev/null 2>&1
}

complete_run_id=""
find_complete_run() {
    local runs_json="$1" run_id artifacts_json complete name live
    complete_run_id=""
    mapfile -t run_ids < <(jq -r \
        '.workflow_runs[]? | select(.status == "completed" and .conclusion == "success") | .id // empty' \
        <<<"$runs_json" 2>/dev/null || true)

    for run_id in "${run_ids[@]}"; do
        artifacts_json=""
        if ! artifacts_json="$("$gh_bin" api \
            "repos/$repo/actions/runs/$run_id/artifacts?per_page=100" 2>/dev/null)"; then
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
    return 1
}

runs_json=""
if ! runs_json="$(fetch_runs)"; then
    decline "prebuild run lookup failed for $sha"
fi

if find_complete_run "$runs_json"; then
    printf 'found=true\n'
    printf 'run-id=%s\n' "$complete_run_id"
    printf 'reason=prebuild run %s carries every release asset for %s\n' "$complete_run_id" "$sha"
    exit 0
fi

if has_pending_run "$runs_json"; then
    deadline=$((SECONDS + wait_seconds))
    while (( SECONDS < deadline )); do
        remaining=$((deadline - SECONDS))
        sleep_for="$poll_seconds"
        if (( sleep_for > remaining )); then
            sleep_for="$remaining"
        fi
        if (( sleep_for > 0 )); then
            sleep "$sleep_for"
        fi

        if ! runs_json="$(fetch_runs)"; then
            decline "prebuild run lookup failed while waiting for run(s) for $sha"
        fi
        if find_complete_run "$runs_json"; then
            printf 'found=true\n'
            printf 'run-id=%s\n' "$complete_run_id"
            printf 'reason=prebuild run %s completed during the bounded wait for %s\n' \
                "$complete_run_id" "$sha"
            exit 0
        fi
        if ! has_pending_run "$runs_json"; then
            break
        fi
    done

    if has_pending_run "$runs_json"; then
        decline "prebuild still queued or in progress after ${wait_seconds}s"
    fi
fi

decline "prebuild runs for $sha have no live copy of every release asset"
