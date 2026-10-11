#!/usr/bin/env bash
# Measure tag push -> published release (cas-3b7c0 / GH #449), and the whole
# wall clock an operator feels: request and first cut -> published, with each
# blocker's time cost (cas-a629).
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
    if [[ "$candidate_count" -eq 1 ]]; then run_dir="$candidate"; fi
fi

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
# The tag push itself is the clock start, so take the FIRST run created for
# this tag. A rerun or a later attempt must never be allowed to shorten the
# measured latency.
tag_pushed_at="$(jq -r '[.workflow_runs[]?.created_at] | sort | first // empty' <<<"$runs_json")"
tag_run_id="$(jq -r '[.workflow_runs[]? | {id, created_at}] | sort_by(.created_at) | first.id // empty' <<<"$runs_json")"
if [[ -z "$tag_pushed_at" ]]; then
    echo "error: no Release workflow run found for $tag; cannot time its publication" >&2
    exit 1
fi

start="$(epoch_of "$tag_pushed_at")" || {
    echo "error: could not parse tag push timestamp $tag_pushed_at" >&2
    exit 1
}
end="$(epoch_of "$published_at")" || {
    echo "error: could not parse publish timestamp $published_at" >&2
    exit 1
}
latency=$((end - start))

within=true
if [[ "$latency" -lt 0 ]]; then
    echo "error: publication ($published_at) precedes the tag push ($tag_pushed_at)" >&2
    exit 1
fi
if [[ "$latency" -gt "$budget" ]]; then
    within=false
fi

printf 'TAG=%s\n' "$tag"
printf 'TAG_RUN_ID=%s\n' "$tag_run_id"
printf 'TAG_PUSHED_AT=%s\n' "$tag_pushed_at"
printf 'PUBLISHED_AT=%s\n' "$published_at"
printf 'PUBLISH_LATENCY_SECONDS=%s\n' "$latency"
printf 'BUDGET_SECONDS=%s\n' "$budget"
printf 'WITHIN_BUDGET=%s\n' "$within"
printf 'INTERVENTIONS=%s\n' "$(intervention_count)"
printf 'BLOCKERS=%s\n' "$(intervention_blockers)"
printf 'GREEN_TO_PIPELINE_SECS=%s\n' "$(epoch_delta "${run_dir:-}/gate.green.epoch" "${run_dir:-}/pipeline.start.epoch")"
printf 'MERGED_TO_PUBLISHER_SECS=%s\n' "$(epoch_delta "${run_dir:-}/pipeline.merged.epoch" "${run_dir:-}/publisher.start.epoch")"
# End to end (cas-a629): request and first cut to publication, and every
# blocker priced from its block to its stage's next completion.
python3 "$(dirname "${BASH_SOURCE[0]}")/release-metrics.py" "${run_dir:-}" "$end"

if ! "$within"; then
    echo "WARN: $tag took ${latency}s from tag push to publication, over the ${budget}s budget" >&2
fi
