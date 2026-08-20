#!/usr/bin/env bash
# Reclaim queued workflow runs that have outlived normal scheduling. Merge
# queue runs have a distinct orphan policy in cas-065a and are intentionally
# excluded here.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=watchdog-policy.sh
source "$script_dir/watchdog-policy.sh"
watchdog_policy_init
repository="$WATCHDOG_REPOSITORY"
queue_seconds="$WATCHDOG_THRESHOLD_SECONDS"
now_epoch="$WATCHDOG_NOW_EPOCH"

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
    watchdog_cancel_run "$run_id"
done < <(jq -r '.workflow_runs[] | [.id, .created_at, .event, .head_branch] | @tsv' <<<"$queued_runs")
