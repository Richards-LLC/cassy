#!/usr/bin/env bash
# Cancel merge-queue workflow runs that exceed twice the observed suite-build
# p95. A vanished queue entry otherwise leaves its self-hosted claim occupied.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=watchdog-policy.sh
source "$script_dir/watchdog-policy.sh"
watchdog_policy_init
repository="$WATCHDOG_REPOSITORY"
hang_seconds="$WATCHDOG_THRESHOLD_SECONDS"
now_epoch="$WATCHDOG_NOW_EPOCH"

in_progress_runs="$(gh api "repos/$repository/actions/runs?event=merge_group&status=in_progress&per_page=100")"
queued_runs="$(gh api "repos/$repository/actions/runs?event=merge_group&status=queued&per_page=100")"
runs="$(printf '%s\n%s\n' "$in_progress_runs" "$queued_runs" | jq -s '{workflow_runs: map(.workflow_runs[]) }')"
archive_job='Fast Validation — suite archive build'
archive_job_state() {
    gh api "repos/$repository/actions/runs/$1/jobs?filter=latest&per_page=100" \
        | jq -r --arg name "$archive_job" \
            '(.jobs | map(select(.name == $name)) | first) as $job
             | if $job == null then empty else [$job.status, ($job.started_at // $job.created_at)] | @tsv end'
}

while IFS=$'\t' read -r run_id workflow_name head_branch run_status; do
    [[ -n "$run_id" && "$workflow_name" == CI ]] || continue
    job_state="$(archive_job_state "$run_id")"
    IFS=$'\t' read -r job_status started_at <<<"$job_state"
    [[ "$job_status" == queued || "$job_status" == in_progress ]] || continue
    if ! started_epoch="$(date -u -d "$started_at" +%s 2>/dev/null)"; then
        printf 'skipping merge_group run=%s with invalid archive time=%s\n' "$run_id" "$started_at" >&2
        continue
    fi
    age_seconds=$((now_epoch - started_epoch))
    (( age_seconds > hang_seconds )) || continue
    current_status="$(gh api "repos/$repository/actions/runs/$run_id" --jq '.status')"
    if [[ "$current_status" != queued && "$current_status" != in_progress ]]; then
        printf 'skipping inactive merge_group run=%s current_status=%s\n' "$run_id" "$current_status"
        continue
    fi
    current_job_state="$(archive_job_state "$run_id")"
    IFS=$'\t' read -r current_job_status _ <<<"$current_job_state"
    if [[ "$current_job_status" != queued && "$current_job_status" != in_progress ]]; then
        printf 'skipping inactive archive job for run=%s current_status=%s\n' "$run_id" "$current_job_status"
        continue
    fi
    printf 'cancelling stale merge_group run=%s status=%s job_status=%s age_seconds=%s threshold_seconds=%s ref=%s\n' \
        "$run_id" "$run_status" "$current_job_status" "$age_seconds" "$hang_seconds" "$head_branch"
    watchdog_cancel_run "$run_id"
done < <(jq -r '.workflow_runs[] | [.id, .name, .head_branch, .status] | @tsv' <<<"$runs")
