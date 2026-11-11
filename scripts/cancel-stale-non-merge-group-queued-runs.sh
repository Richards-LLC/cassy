#!/usr/bin/env bash
# Reclaim queued workflow runs that have outlived normal scheduling. Merge
# queue runs have a distinct orphan policy in cas-065a and are intentionally
# excluded here.
set -euo pipefail

# Every failed script exit emits its registered check codes, including early
# argument errors and failures propagated from child commands.
gate_exit() {
    local status="$1"; shift
    if (( status != 0 )); then printf '\n%s\n' "$@" >&2; fi
}
trap 'gate_exit "$?" "[W-63]"' EXIT


script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=watchdog-policy.sh
source "$script_dir/watchdog-policy.sh"
watchdog_policy_init
repository="$WATCHDOG_REPOSITORY"
queue_seconds="$WATCHDOG_THRESHOLD_SECONDS"
now_epoch="$WATCHDOG_NOW_EPOCH"
selected=0 cancel_accepted=0 cancel_rejected=0 unresolved=0 status_failed=0 dry_run=0 unverified=0

list_call="repos/$repository/actions/runs?status=queued&per_page=100"
if ! queued_runs="$(watchdog_api_read "$list_call")"; then
    printf 'error: queued list failed after %s attempts call=GET %s; cannot enumerate queued runs\n' \
        "$WATCHDOG_API_READ_ATTEMPTS" "$list_call" >&2
    exit 1
fi
while IFS=$'\t' read -r run_id attempt_started_at event head_branch; do
    [[ -n "$run_id" && "$attempt_started_at" != null ]] || continue
    # cas-065a owns merge_group queue/orphan reclamation. Keeping that event
    # out of this broader stale-queue sweep prevents duelling cancellers.
    [[ "$event" != merge_group ]] || continue
    if ! attempt_epoch="$(watchdog_timestamp_epoch "$attempt_started_at" 2>/dev/null)"; then
        printf 'skipping queued run=%s with invalid attempt timestamp=%s\n' "$run_id" "$attempt_started_at" >&2
        continue
    fi
    age_seconds=$((now_epoch - attempt_epoch))
    # gate[W-63 watchdog]: GitHub stale queued-run watchdog
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
                    # GitHub can retain queued ghosts that reject cancellation
                    # indefinitely. Keep each visible without a red run every
                    # five minutes; an unreadable status still fails the sweep.
                    [[ "$current_status" == queued ]] || unverified=$((unverified + 1))
                    detail="unresolved stale queued run=$run_id current_status=$current_status after rejected cancellation; inspect https://github.com/$repository/actions/runs/$run_id"
                    printf '::warning::%s\n' "$detail" >&2
                    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
                        printf '%s\n\n' "$detail" >>"$GITHUB_STEP_SUMMARY"
                    fi
                    ;;
            esac
            ;;
    esac
done < <(jq -r '.workflow_runs[] | [.id,
    # created_at belongs to the original run, even after a re-run. Only a
    # first attempt may fall back to it when run_started_at is unavailable.
    ((if (.run_attempt // 1) > 1 then .run_started_at
      else (.run_started_at // .created_at) end) // "null"),
    .event, .head_branch] | @tsv' <<<"$queued_runs")

# A 202 acknowledges a request; it does not prove that any run was reclaimed.
summary="sweep complete: selected=$selected cancel_accepted=$cancel_accepted cancel_rejected=$cancel_rejected unresolved=$unresolved status_failed=$status_failed dry_run=$dry_run"
printf '%s\nCancellation acceptance does not verify reclamation.\n' "$summary"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    printf '%s\n\nCancellation acceptance does not verify reclamation.\n' "$summary" >>"$GITHUB_STEP_SUMMARY"
fi
(( unverified == 0 ))
