#!/usr/bin/env bash

cut_stage_pr_body() {
    if cut_has_external_stage pr-body; then
        cut_run_external_stage pr-body
        return
    fi
    local changelog="$worktree/CHANGELOG.md" output="$run_dir/pr-body.md"
    local -a review_args=(
        --version "$version" --changelog "$changelog"
        --gate-log "$run_dir/gate.log" --output "$output"
    )
    # These declarations are display-only copies from the release task.
    # Missing declarations are shown explicitly, never inferred as safe.
    if [[ -n "${CAS_RELEASE_TRAIN_TASK_RISK:-}" ]]; then
        review_args+=(--risk "$CAS_RELEASE_TRAIN_TASK_RISK")
    fi
    if [[ -n "${CAS_RELEASE_TRAIN_TASK_DOOR:-}" ]]; then
        review_args+=(--door "$CAS_RELEASE_TRAIN_TASK_DOOR")
    fi
    local helper_dir="${script_dir:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)}"
    python3 "$helper_dir/review-pr-body.py" "${review_args[@]}"
}
