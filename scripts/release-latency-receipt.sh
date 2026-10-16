#!/usr/bin/env bash
# Measure the publication start -> published release. Legacy tag-push releases
# retain their original endpoint; main-push releases use merge-queue green, or
# main landing when merge-queue evidence is unavailable (cas-fab8). Also record
# the whole wall clock an operator feels: request and first cut -> published,
# with each blocker's time cost (cas-a629).
#
# The digest receipt (scripts/release-published-receipt.sh) proves *what* was
# published. This proves *how fast*. Both are needed before a release is
# announced as fast: a receipt that only reports digests cannot tell a
# 3-minute prebuilt publication from a 26-minute cold one.
#
# An overrun is evidence, not a reason to strand an already-published release.
# Missing or incoherent measurements still fail; overruns warn and record false.
set -euo pipefail

# shellcheck source=scripts/release-portable.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-portable.sh"

usage() {
    echo "Usage: scripts/release-latency-receipt.sh <vX.Y.Z> [--budget-seconds <n>] [--run-dir <path>]" >&2
}

if [[ "$#" -lt 1 ]]; then
    usage
    exit 2
fi

tag="$1"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: expected an annotated release tag like vX.Y.Z; got $tag" >&2
    exit 2
fi

budget=600
run_dir="${CAS_RELEASE_TRAIN_RUN_DIR:-${CAS_RELEASE_RUN_DIR:-}}"
# The train's post-publication stage owns its atomic receipt hand-off. A
# standalone measurement writes into its selected artifact run directory so
# `cas release report` can consume it without a separate copy step.
persist_receipt=true
if [[ -n "${CAS_RELEASE_TRAIN_RUN_DIR:-}" ]]; then persist_receipt=false; fi
shift
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --budget-seconds)
            [[ "$#" -ge 2 && "$2" =~ ^[0-9]+$ ]] || { usage; exit 2; }
            budget="$2"
            shift 2
            ;;
        --run-dir)
            [[ "$#" -ge 2 && -n "$2" ]] || { usage; exit 2; }
            run_dir="$2"
            shift 2
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$run_dir" ]]; then
    artifacts_root="${CAS_RELEASE_ARTIFACTS_ROOT:-$HOME/.cas/artifacts/release}"
    candidate_count=0
    candidate=''
    while IFS= read -r candidate_path; do
        candidate="$candidate_path"
        candidate_count=$((candidate_count + 1))
    done < <(find "$artifacts_root" -mindepth 1 -maxdepth 1 -type d \
        -name "v${tag#v}-*" -print 2>/dev/null | sort)
    if [[ "$candidate_count" -eq 1 ]]; then
        run_dir="$candidate"
    else
        # A standalone measurement may have no release-train run directory.
        # Give the report consumer a stable place to find this receipt.
        run_dir="$artifacts_root/$tag-latency"
    fi
fi

receipt_output="$(mktemp)"
trap 'rm -f "$receipt_output"' EXIT

intervention_count() {
    python3 "$(dirname "${BASH_SOURCE[0]}")/release-interventions.py" count "$run_dir"
}

intervention_blockers() {
    python3 "$(dirname "${BASH_SOURCE[0]}")/release-interventions.py" stages "$run_dir"
}

epoch_delta() {
    local start_file="$1" end_file="$2" start end
    if [[ -r "$start_file" ]]; then start="$(tr -d '[:space:]' <"$start_file")"; else start=''; fi
    if [[ -r "$end_file" ]]; then end="$(tr -d '[:space:]' <"$end_file")"; else end=''; fi
    if [[ "$start" =~ ^[0-9]+$ && "$end" =~ ^[0-9]+$ && "$end" -ge "$start" ]]; then
        printf '%s\n' "$((end - start))"
    else
        printf 'unavailable\n'
    fi
}

gh_bin="${GH_BIN:-gh}"
repo="${RELEASE_REPO:-Richards-LLC/cassy}"

# Keep GNU date results unchanged; BSD date requires the ISO fallback.
epoch_of() {
    release_portable_timestamp_epoch "$1"
}

published_at="$("$gh_bin" release view "$tag" --repo "$repo" --json publishedAt --jq '.publishedAt // empty')"
if [[ -z "$published_at" ]]; then
    echo "error: release $tag is not published yet" >&2
    exit 1
fi

runs_json="$("$gh_bin" api "repos/$repo/actions/workflows/release.yml/runs?branch=$tag&event=push&per_page=50")"
# Keep the first run for legacy tag-push releases, so reruns cannot shorten
# their measured latency. The current release workflow starts on main and
# creates the tag during publication; in that case match the tag's commit to
# the first main-push run for that exact SHA instead of calling it tag latency.
tag_run_count="$(jq -r '[.workflow_runs[]?] | length' <<<"$runs_json")"
start_kind=''
start_at=''
start_description='tag push'
run_id=''
tag_pushed_at=''
tag_run_id=''
main_push_run_created_at=''
main_push_run_id=''
main_landed_at=''
main_landed_epoch=''
gate_green_at=''
gate_green_epoch=''
gate_green_run_id=''
if [[ "$tag_run_count" -gt 0 ]]; then
    tag_run="$(jq -c '[.workflow_runs[]? | {id, created_at}] | sort_by(.created_at) | first // empty' <<<"$runs_json")"
    tag_pushed_at="$(jq -r '.created_at // empty' <<<"$tag_run")"
    tag_run_id="$(jq -r '.id // empty' <<<"$tag_run")"
    start_kind='tag-push'
    start_at="$tag_pushed_at"
    run_id="$tag_run_id"
else
    release_sha=''
    if ! release_sha="$("$gh_bin" api "repos/$repo/commits/$tag" --jq '.sha // empty')"; then
        release_sha=''
    fi
    if [[ ! "$release_sha" =~ ^[0-9a-f]{40}$ ]]; then
        echo "error: cannot resolve commit for release tag $tag; cannot time its publication" >&2
        exit 1
    fi
    pr_json="$("$gh_bin" api "repos/$repo/commits/$release_sha/pulls")"
    release_pr="$(jq -c --arg sha "$release_sha" \
        '[.[]? | select(.base.ref == "main" and .merged_at != null and .merge_commit_sha == $sha) | {number, merged_at}] | sort_by(.merged_at) | first // empty' \
        <<<"$pr_json")"
    pr_number="$(jq -r '.number // empty' <<<"$release_pr")"
    main_landed_at="$(jq -r '.merged_at // empty' <<<"$release_pr")"
    if [[ -z "$pr_number" || -z "$main_landed_at" ]]; then
        echo "error: no merged main PR found for $tag commit $release_sha; cannot time its main landing" >&2
        exit 1
    fi
    main_landed_epoch="$(epoch_of "$main_landed_at")" || {
        echo "error: could not parse main landing timestamp $main_landed_at" >&2
        exit 1
    }
    main_runs_json="$("$gh_bin" api "repos/$repo/actions/workflows/release.yml/runs?branch=main&event=push&head_sha=$release_sha&per_page=100")"
    main_run="$(jq -c --arg sha "$release_sha" \
        '[.workflow_runs[]? | select(.event == "push" and .head_branch == "main" and .head_sha == $sha) | {id, created_at}] | sort_by(.created_at) | first // empty' \
        <<<"$main_runs_json")"
    main_push_run_created_at="$(jq -r '.created_at // empty' <<<"$main_run")"
    main_push_run_id="$(jq -r '.id // empty' <<<"$main_run")"
    if [[ -z "$main_push_run_created_at" || -z "$main_push_run_id" ]]; then
        echo "error: no Release workflow run found on main for $tag commit $release_sha; cannot time its publication" >&2
        exit 1
    fi
    merge_group_runs_json="$("$gh_bin" api "repos/$repo/actions/runs?event=merge_group&head_sha=$release_sha&per_page=100")"
    merge_group_run="$(jq -c --arg sha "$release_sha" --arg pr "$pr_number" --arg merged "$main_landed_at" \
        '[.workflow_runs[]? | select(.event == "merge_group" and .name == "CI" and .head_sha == $sha and .status == "completed" and .conclusion == "success" and .updated_at <= $merged and (.head_branch | contains("/pr-" + $pr + "-"))) | {id, updated_at}] | sort_by(.updated_at) | first // empty' \
        <<<"$merge_group_runs_json")"
    gate_green_at="$(jq -r '.updated_at // empty' <<<"$merge_group_run")"
    gate_green_run_id="$(jq -r '.id // empty' <<<"$merge_group_run")"
    if [[ -n "$gate_green_at" ]]; then
        gate_green_epoch="$(epoch_of "$gate_green_at")" || {
            echo "error: could not parse merge-queue CI completion timestamp $gate_green_at" >&2
            exit 1
        }
        start_kind='gate-green'
        start_at="$gate_green_at"
        start_description='merge-queue CI green'
    else
        # A successful merge-queue CI run is preferred as the release gate's
        # green moment. Older merges may lack that run, so report a separately
        # named main-landing interval instead of fabricating gate-green data.
        start_kind='main-landed'
        start_at="$main_landed_at"
        start_description='main landing'
    fi
    run_id="$main_push_run_id"
fi
if [[ -z "$start_at" || -z "$run_id" ]]; then
    echo "error: Release workflow run for $tag is missing its start time or run id" >&2
    exit 1
fi

start="$(epoch_of "$start_at")" || {
    echo "error: could not parse $start_description timestamp $start_at" >&2
    exit 1
}
end="$(epoch_of "$published_at")" || {
    echo "error: could not parse publish timestamp $published_at" >&2
    exit 1
}
latency=$((end - start))

within=true
if [[ "$latency" -lt 0 ]]; then
    echo "error: publication ($published_at) precedes the $start_description timestamp ($start_at)" >&2
    exit 1
fi
if [[ "$latency" -gt "$budget" ]]; then
    within=false
fi

if [[ -n "$run_dir" && "$start_kind" != tag-push ]]; then
    mkdir -p "$run_dir"
    main_landed_tmp="$(mktemp "$run_dir/.main.landed.epoch.XXXXXX")"
    printf '%s\n' "$main_landed_epoch" >"$main_landed_tmp"
    mv "$main_landed_tmp" "$run_dir/main.landed.epoch"
    if [[ -n "$gate_green_epoch" ]]; then
        gate_green_tmp="$(mktemp "$run_dir/.gate.green.epoch.XXXXXX")"
        printf '%s\n' "$gate_green_epoch" >"$gate_green_tmp"
        mv "$gate_green_tmp" "$run_dir/gate.green.epoch"
    fi
fi

{
    printf 'TAG=%s\n' "$tag"
    printf 'PUBLISH_START_KIND=%s\n' "$start_kind"
    printf 'PUBLISH_START_AT=%s\n' "$start_at"
    printf 'PUBLISH_RUN_ID=%s\n' "$run_id"
    if [[ "$start_kind" == tag-push ]]; then
        # Preserve the existing field names for tag-triggered releases.
        printf 'TAG_RUN_ID=%s\n' "$tag_run_id"
        printf 'TAG_PUSHED_AT=%s\n' "$tag_pushed_at"
    else
        printf 'MAIN_PUSH_RUN_ID=%s\n' "$main_push_run_id"
        printf 'MAIN_PUSH_RUN_CREATED_AT=%s\n' "$main_push_run_created_at"
        printf 'MAIN_LANDED_AT=%s\n' "$main_landed_at"
        printf 'MAIN_LANDED_EPOCH=%s\n' "$main_landed_epoch"
        if [[ -n "$gate_green_epoch" ]]; then
            printf 'GATE_GREEN_AT=%s\n' "$gate_green_at"
            printf 'GATE_GREEN_EPOCH=%s\n' "$gate_green_epoch"
            printf 'GATE_GREEN_RUN_ID=%s\n' "$gate_green_run_id"
        fi
    fi
    printf 'PUBLISHED_AT=%s\n' "$published_at"
    printf 'PUBLISH_LATENCY_SECONDS=%s\n' "$latency"
    printf 'BUDGET_SECONDS=%s\n' "$budget"
    printf 'WITHIN_BUDGET=%s\n' "$within"
    printf 'INTERVENTIONS=%s\n' "$(intervention_count)"
    printf 'BLOCKERS=%s\n' "$(intervention_blockers)"
    printf 'GREEN_TO_PIPELINE_SECS=%s\n' "$(epoch_delta "${run_dir:-}/gate.green.epoch" "${run_dir:-}/pipeline.start.epoch")"
    printf 'MERGED_TO_PUBLISHER_SECS=%s\n' "$(epoch_delta "${run_dir:-}/pipeline.merged.epoch" "${run_dir:-}/publisher.start.epoch")"
} >"$receipt_output"
# End to end (cas-a629): request and first cut to publication, and every
# blocker priced from its block to its stage's next completion.
python3 "$(dirname "${BASH_SOURCE[0]}")/release-metrics.py" "${run_dir:-}" "$end" >>"$receipt_output"

if [[ "$persist_receipt" == true && -n "$run_dir" ]]; then
    mkdir -p "$run_dir"
    receipt_tmp="$(mktemp "$run_dir/.release-latency.receipt.XXXXXX")"
    cat "$receipt_output" >"$receipt_tmp"
    mv "$receipt_tmp" "$run_dir/release-latency.receipt"
fi
cat "$receipt_output"

if ! "$within"; then
    echo "WARN: $tag took ${latency}s from $start_description to publication, over the ${budget}s budget" >&2
fi
