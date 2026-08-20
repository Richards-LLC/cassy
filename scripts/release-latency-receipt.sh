#!/usr/bin/env bash
# Measure the only release number an operator feels: tag push -> published
# release (cas-3b7c0 / GH #449).
#
# The digest receipt (scripts/release-published-receipt.sh) proves *what* was
# published. This proves *how fast*. Both are needed before a release is
# announced as fast: a receipt that only reports digests cannot tell a
# 3-minute prebuilt publication from a 26-minute cold one.
#
# Exits non-zero when the measured latency exceeds the budget, so the number is
# a gate an operator can run rather than a figure to eyeball.
set -euo pipefail

usage() {
    echo "Usage: scripts/release-latency-receipt.sh <vX.Y.Z> [--budget-seconds <n>]" >&2
}

if [[ "$#" -ne 1 && "$#" -ne 3 ]]; then
    usage
    exit 2
fi

tag="$1"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: expected an annotated release tag like vX.Y.Z; got $tag" >&2
    exit 2
fi

budget=600
if [[ "$#" -eq 3 ]]; then
    if [[ "$2" != "--budget-seconds" || ! "$3" =~ ^[0-9]+$ ]]; then
        usage
        exit 2
    fi
    budget="$3"
fi

gh_bin="${GH_BIN:-gh}"
repo="${RELEASE_REPO:-Richards-LLC/cassy}"

epoch_of() {
    date -u -d "$1" +%s 2>/dev/null || return 1
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

if ! "$within"; then
    echo "error: $tag took ${latency}s from tag push to publication, over the ${budget}s budget" >&2
    exit 1
fi
