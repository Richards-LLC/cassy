#!/usr/bin/env bash
# Choose the runner for one release prebuild platform (release-prebuild.yml).
#
# GitHub never reassigns a job queued on an offline self-hosted runner, so a
# trusted build first dispatches a no-op probe run on the same ref and routes
# to the self-hosted runner only if that runner starts the probe in time.
# Every other case (an untrusted ref, release.yml's fallback call, a busy or
# offline runner, an API error) builds on the hosted runner. Trusted means a
# merge-queue ref of main or a direct push to main; the workflow has no
# pull_request trigger.
#
# Environment: GH_TOKEN, REPO, EVENT_NAME, REF, REF_NAME, PLATFORM (the
# workflow's platform input), PLATFORM_LABEL (for messages), PROBE (the
# platform input that runs the probe job), PROBE_JOB (its job name),
# SELF_HOSTED_RUNNER and HOSTED_RUNNER (runs-on JSON). Writes runner= and
# mode= to GITHUB_OUTPUT.
set -euo pipefail

route() {
    printf 'runner=%s\nmode=%s\n' "$1" "$2" >> "$GITHUB_OUTPUT"
    echo "$PLATFORM_LABEL prebuild runner: $2" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
    exit 0
}

case "$EVENT_NAME:$REF:$PLATFORM" in
    merge_group:refs/heads/gh-readonly-queue/main/*: | push:refs/heads/main:) ;;
    *) route "$HOSTED_RUNNER" hosted ;;
esac
if ! run_id="$(gh api -X POST "repos/$REPO/actions/workflows/release-prebuild.yml/dispatches" \
        -f ref="$REF_NAME" -f "inputs[platform]=$PROBE" -F return_run_details=true \
        --jq .workflow_run_id)" || [[ -z "$run_id" ]]; then
    echo "::warning::$PLATFORM_LABEL runner probe dispatch failed; building hosted"
    route "$HOSTED_RUNNER" hosted
fi
deadline=$((SECONDS + ${PROBE_WAIT_SECONDS:-90}))
while (( SECONDS < deadline )); do
    state="$(gh api "repos/$REPO/actions/runs/$run_id/jobs" \
        --jq ".jobs[] | select(.name == \"$PROBE_JOB\") | \"\(.status):\(.conclusion)\"" || true)"
    case "$state" in
        in_progress:* | completed:success) route "$SELF_HOSTED_RUNNER" self-hosted ;;
    esac
    sleep "${PROBE_POLL_SECONDS:-5}"
done
gh api -X POST "repos/$REPO/actions/runs/$run_id/cancel" > /dev/null || true
echo "::warning::No $PLATFORM_LABEL release runner started the probe; building hosted"
route "$HOSTED_RUNNER" hosted
