#!/usr/bin/env bash
# Cancel merge-queue workflow runs that exceed twice the observed suite-build
# p95. A vanished queue entry otherwise leaves its self-hosted claim occupied.
set -euo pipefail

repository="${GITHUB_REPOSITORY:-Richards-LLC/cassy}"
hang_seconds="${CASSY_MERGE_GROUP_HANG_SECONDS:-1200}"
now_epoch="${CASSY_NOW_EPOCH:-$(date -u +%s)}"

if [[ ! "$repository" =~ ^[^/]+/[^/]+$ ]]; then
    echo "GITHUB_REPOSITORY must be an owner/repository slug" >&2
    exit 2
fi
if [[ ! "$hang_seconds" =~ ^[0-9]+$ ]] || (( hang_seconds == 0 )); then
    echo "CASSY_MERGE_GROUP_HANG_SECONDS must be a positive integer" >&2
    exit 2
fi
if [[ ! "$now_epoch" =~ ^[0-9]+$ ]]; then
    echo "CASSY_NOW_EPOCH must be epoch seconds" >&2
    exit 2
fi

runs="$(gh api "repos/$repository/actions/runs?event=merge_group&status=in_progress&per_page=100")"
while IFS=$'\t' read -r run_id started_at head_branch; do
    [[ -n "$run_id" && "$started_at" != null ]] || continue
    started_epoch="$(date -u -d "$started_at" +%s)"
    age_seconds=$((now_epoch - started_epoch))
    (( age_seconds > hang_seconds )) || continue
    printf 'cancelling stale merge_group run=%s age_seconds=%s threshold_seconds=%s ref=%s\n' \\
        "$run_id" "$age_seconds" "$hang_seconds" "$head_branch"
    gh api --method POST "repos/$repository/actions/runs/$run_id/cancel" >/dev/null
done < <(jq -r '.workflow_runs[] | [.id, (.run_started_at // .created_at), .head_branch] | @tsv' <<<"$runs")
