#!/usr/bin/env bash
# Reclaim queued workflow runs that have outlived normal scheduling. Merge
# queue runs have a distinct orphan policy in cas-065a and are intentionally
# excluded here.
set -euo pipefail

repository="${GITHUB_REPOSITORY:-Richards-LLC/cassy}"
queue_seconds="${CASSY_NON_MERGE_GROUP_QUEUE_SECONDS:-1200}"
now_epoch="${CASSY_NOW_EPOCH:-$(date -u +%s)}"

if [[ ! "$repository" =~ ^[^/]+/[^/]+$ ]]; then
    echo "GITHUB_REPOSITORY must be an owner/repository slug" >&2
    exit 2
fi
if [[ ! "$queue_seconds" =~ ^[0-9]+$ ]] || (( queue_seconds == 0 )); then
    echo "CASSY_NON_MERGE_GROUP_QUEUE_SECONDS must be a positive integer" >&2
    exit 2
fi
if [[ ! "$now_epoch" =~ ^[0-9]+$ ]]; then
    echo "CASSY_NOW_EPOCH must be epoch seconds" >&2
    exit 2
fi

queued_runs="$(gh api "repos/$repository/actions/runs?status=queued&per_page=100")"
while IFS=$'\t' read -r run_id created_at event head_branch; do
    [[ -n "$run_id" && "$created_at" != null ]] || continue
    # cas-065a owns merge_group queue/orphan reclamation. Keeping that event
    # out of this broader stale-queue sweep prevents duelling cancellers.
    [[ "$event" != merge_group ]] || continue
    if ! created_epoch="$(date -u -d "$created_at" +%s 2>/dev/null)"; then
        printf 'skipping queued run=%s with invalid created_at=%s\n' "$run_id" "$created_at" >&2
        continue
    fi
    age_seconds=$((now_epoch - created_epoch))
    (( age_seconds > queue_seconds )) || continue

    # The list endpoint is eventually consistent. Re-read status so a run
    # claimed or cancelled between list and action is never cancelled again.
    current_status="$(gh api "repos/$repository/actions/runs/$run_id" --jq '.status')"
    if [[ "$current_status" != queued ]]; then
        printf 'skipping no-longer-queued run=%s current_status=%s\n' "$run_id" "$current_status"
        continue
    fi

    printf 'cancelling stale queued run=%s event=%s age_seconds=%s threshold_seconds=%s ref=%s\n' \
        "$run_id" "$event" "$age_seconds" "$queue_seconds" "$head_branch"
    gh api --method POST "repos/$repository/actions/runs/$run_id/cancel" >/dev/null
done < <(jq -r '.workflow_runs[] | [.id, .created_at, .event, .head_branch] | @tsv' <<<"$queued_runs")
