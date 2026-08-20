#!/usr/bin/env bash
# Shared safety policy for GitHub Actions watchdogs. Successful suite archive
# builds measured 288–576s, so this 20-minute limit stays above twice that p95.
# Keep the timeout here so each watchdog uses the same reviewed value instead
# of drifting independently.

watchdog_policy_init() {
    WATCHDOG_REPOSITORY="${GITHUB_REPOSITORY:-}"
    WATCHDOG_THRESHOLD_SECONDS="${CASSY_WATCHDOG_STALE_SECONDS:-1200}"
    WATCHDOG_NOW_EPOCH="${CASSY_NOW_EPOCH:-$(date -u +%s)}"
    WATCHDOG_DRY_RUN="${CASSY_WATCHDOG_DRY_RUN:-false}"

    if [[ ! "$WATCHDOG_REPOSITORY" =~ ^[^/]+/[^/]+$ ]]; then
        echo "GITHUB_REPOSITORY must be an explicit owner/repository slug" >&2
        exit 2
    fi
    if [[ ! "$WATCHDOG_THRESHOLD_SECONDS" =~ ^[0-9]+$ ]] || (( WATCHDOG_THRESHOLD_SECONDS == 0 )); then
        echo "CASSY_WATCHDOG_STALE_SECONDS must be a positive integer" >&2
        exit 2
    fi
    if [[ ! "$WATCHDOG_NOW_EPOCH" =~ ^[0-9]+$ ]]; then
        echo "CASSY_NOW_EPOCH must be epoch seconds" >&2
        exit 2
    fi
    if [[ "$WATCHDOG_DRY_RUN" != true && "$WATCHDOG_DRY_RUN" != false ]]; then
        echo "CASSY_WATCHDOG_DRY_RUN must be true or false" >&2
        exit 2
    fi
}

watchdog_cancel_run() {
    local run_id="$1"
    if [[ "$WATCHDOG_DRY_RUN" == true ]]; then
        printf 'dry-run would cancel run=%s\n' "$run_id"
    elif ! gh api --method POST "repos/$WATCHDOG_REPOSITORY/actions/runs/$run_id/cancel" >/dev/null; then
        # A run can complete after its final status read.  Do not abandon the
        # sweep merely because GitHub rejects that now-stale cancellation.
        printf 'unable to cancel run=%s; it may already be complete\n' "$run_id" >&2
    fi
}
