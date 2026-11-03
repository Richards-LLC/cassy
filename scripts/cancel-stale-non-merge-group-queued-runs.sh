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
selected=0 cancel_accepted=0 cancel_rejected=0 unresolved=0 status_failed=0 dry_run=0

list_call="repos/$repository/actions/runs?status=queued&per_page=100"
if ! queued_runs="$(watchdog_api_read "$list_call")"; then
    printf 'error: queued list failed after %s attempts call=GET %s; cannot enumerate queued runs\n' \
        "$WATCHDOG_API_READ_ATTEMPTS" "$list_call" >&2
    exit 1
fi
while IFS=$'\t' read -r run_id created_at event head_branch; do
    [[ -n "$run_id" && "$created_at" != null ]] || continue
    # cas-065a owns merge_group queue/orphan reclamation. Keeping that event
    # out of this broader stale-queue sweep prevents duelling cancellers.
    [[ "$event" != merge_group ]] || continue
    if ! created_epoch="$(watchdog_timestamp_epoch "$created_at" 2>/dev/null)"; then
        printf 'skipping queued run=%s with invalid created_at=%s\n' "$run_id" "$created_at" >&2
        continue
    fi
    age_seconds=$((now_epoch - created_epoch))
    (( age_seconds > queue_seconds )) || continue

    # The list endpoint is eventually consistent. Re-read status so a run
    # claimed or cancelled between list and action is never cancelled again.
    status_call="repos/$repository/actions/runs/$run_id"
    if ! current_status="$(watchdog_api_read "$status_call" --jq '.status')"; then
        printf 'warning: status failed after %s attempts run=%s call=GET %s; skipping run\n' \
            "$WATCHDOG_API_READ_ATTEMPTS" "$run_id" "$status_call" >&2
        status_failed=$((status_failed + 1))
        continue
    fi
    if [[ "$current_status" != queued ]]; then
        printf 'skipping no-longer-queued run=%s current_status=%s\n' "$run_id" "$current_status"
        continue
    fi

    printf 'cancelling stale queued run=%s event=%s age_seconds=%s threshold_seconds=%s ref=%s\n' \
        "$run_id" "$event" "$age_seconds" "$queue_seconds" "$head_branch"
    selected=$((selected + 1))
    watchdog_cancel_run "$run_id"
    case "$WATCHDOG_CANCEL_RESULT" in
        accepted) cancel_accepted=$((cancel_accepted + 1)) ;;
        dry_run) dry_run=$((dry_run + 1)) ;;
        rejected)
            cancel_rejected=$((cancel_rejected + 1))
            # A rejected POST is harmless if the run has since left the queue.
            # Only read here: never force-cancel, delete or retry cancellation.
            if ! current_status="$(watchdog_api_read "$status_call" --jq '.status')"; then
                current_status=unknown
                status_failed=$((status_failed + 1))
            fi
            case "$current_status" in
                completed|in_progress|waiting|pending|requested)
                    printf 'cancel rejected but run=%s left queue current_status=%s\n' "$run_id" "$current_status"
                    ;;
                *)
                    unresolved=$((unresolved + 1))
                    printf '::error::unresolved stale queued run=%s current_status=%s after rejected cancellation; inspect https://github.com/%s/actions/runs/%s\n' \
                        "$run_id" "$current_status" "$repository" "$run_id" >&2
                    ;;
            esac
            ;;
    esac
done < <(jq -r '.workflow_runs[] | [.id, .created_at, .event, .head_branch] | @tsv' <<<"$queued_runs")

# A 202 acknowledges a request; it does not prove that any run was reclaimed.
summary="sweep complete: selected=$selected cancel_accepted=$cancel_accepted cancel_rejected=$cancel_rejected unresolved=$unresolved status_failed=$status_failed dry_run=$dry_run"
printf '%s\nCancellation acceptance does not verify reclamation.\n' "$summary"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '%s\n\nCancellation acceptance does not verify reclamation.\n' "$summary" >>"$GITHUB_STEP_SUMMARY"
fi
(( unresolved == 0 ))
