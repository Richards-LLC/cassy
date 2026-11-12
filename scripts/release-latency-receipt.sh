#!/usr/bin/env bash
# One receipt-backed clock. Retries never reset the cut or delivery merge.
# Missing endpoints remain unavailable; overruns do not strand publication.
set -euo pipefail
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/release-portable.sh"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
usage() { echo "Usage: scripts/release-latency-receipt.sh <vX.Y.Z> [--budget-seconds <n>] [--run-dir <path>]" >&2; }
[[ $# -ge 1 && "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { usage; exit 2; }
tag="$1"; shift
budget="$(jq -er '.budgets.publish' "$script_dir/release-stage-budgets.json")"
run_dir="${CAS_RELEASE_TRAIN_RUN_DIR:-${CAS_RELEASE_RUN_DIR:-}}"
while (($#)); do
    case "$1" in
        --budget-seconds) [[ $# -ge 2 && "$2" =~ ^[0-9]+$ ]] || { usage; exit 2; }; budget="$2"; shift 2 ;;
        --run-dir) [[ $# -ge 2 && -n "$2" ]] || { usage; exit 2; }; run_dir="$2"; shift 2 ;;
        *) usage; exit 2 ;;
    esac
done
if [[ -z "$run_dir" ]]; then
    artifacts_root="${CAS_RELEASE_ARTIFACTS_ROOT:-$HOME/.cas/artifacts/release}"
    candidates=()
    while IFS= read -r path; do candidates+=("$path"); done < <(
        find "$artifacts_root" -mindepth 1 -maxdepth 1 -type d -name "$tag-*" -print 2>/dev/null | sort)
    if [[ ${#candidates[@]} == 1 ]]; then run_dir="${candidates[0]}"; else run_dir="$artifacts_root/$tag-latency"; fi
fi
gh_bin="${GH_BIN:-gh}"
repo="${RELEASE_REPO:-Richards-LLC/cassy}"
published_at="$("$gh_bin" release view "$tag" --repo "$repo" --json publishedAt --jq '.publishedAt // empty')"
[[ -n "$published_at" ]] || { echo "error: release $tag is not published yet" >&2; exit 1; }
release_sha="$("$gh_bin" api "repos/$repo/commits/$tag" --jq '.sha // empty')"
[[ "$release_sha" =~ ^[0-9a-f]{40}$ ]] || { echo 'error: release commit unavailable' >&2; exit 1; }
pr_json="$("$gh_bin" api "repos/$repo/commits/$release_sha/pulls")"
pr="$(jq -c --arg sha "$release_sha" '[.[]? | select(.base.ref == "main" and .merged_at != null and .merge_commit_sha == $sha)] | sort_by(.merged_at) | first // {}' <<<"$pr_json")"
main_at="$(jq -r '.merged_at // empty' <<<"$pr")"
[[ -n "$main_at" ]] || { echo 'error: actual main landing unavailable' >&2; exit 1; }
main_epoch="$(release_portable_timestamp_epoch "$main_at")" || { echo 'error: actual main landing unavailable' >&2; exit 1; }
end="$(release_portable_timestamp_epoch "$published_at")"
(( end >= main_epoch )) || { echo 'error: publication precedes main landing' >&2; exit 1; }
latency=$((end - main_epoch))
within=true
if (( latency > budget )); then within=false; fi
context="$("${CAS_BIN:-cas}" release context --version "${tag#v}" 2>/dev/null || true)"
if ! jq -e 'type == "object"' <<<"$context" >/dev/null 2>&1; then context='{}'; fi
body="$("$gh_bin" api "repos/$repo/releases/tags/$tag" --jq .body 2>/dev/null || true)"
clock="$(jq -cn --argjson context "$context" --argjson pr "$pr" --arg published_at "$published_at" --arg body "$body" '{context:$context,pr:$pr,published_at:$published_at,body:$body}')"
mkdir -p "$run_dir"
printf '%s\n' "$main_epoch" >"$run_dir/main.landed.epoch"
receipt="$(mktemp "$run_dir/.latency.XXXXXX")"
trap 'rm -f "$receipt"' EXIT
{
    printf 'TAG=%s\nPUBLISH_START_KIND=main-landed\nPUBLISH_START_AT=%s\n' "$tag" "$main_at"
    printf 'MAIN_LANDED_AT=%s\nMAIN_LANDED_EPOCH=%s\nPUBLISHED_AT=%s\n' "$main_at" "$main_epoch" "$published_at"
    printf 'MAIN_TO_PUBLISHED_SECONDS=%s\nMAIN_TO_PUBLISHED_BUDGET_SECONDS=%s\n' "$latency" "$budget"
    # Retain machine-consumer compatibility; no second human timing clock.
    printf 'PUBLISH_LATENCY_SECONDS=%s\nBUDGET_SECONDS=%s\nWITHIN_BUDGET=%s\n' "$latency" "$budget" "$within"
    printf 'INTERVENTIONS=%s\n' "$(python3 "$script_dir/release-interventions.py" count "$run_dir")"
    printf 'BLOCKERS=%s\n' "$(python3 "$script_dir/release-interventions.py" stages "$run_dir")"
    RELEASE_CLOCK_INPUT="$clock" python3 "$script_dir/release-metrics.py" "$run_dir" "$end"
} >"$receipt"
cat "$receipt"
if [[ -z "${CAS_RELEASE_TRAIN_RUN_DIR:-}" ]]; then mv "$receipt" "$run_dir/release-latency.receipt"; fi
if [[ "$within" == false ]]; then echo "WARN: main to published ${latency}s exceeds ${budget}s" >&2; fi
