#!/usr/bin/env bash

cut_stage_pipeline() {
    if cut_has_external_stage pipeline; then
        cut_run_external_stage pipeline
    else
        CAS_RELEASE_TRAIN_INVOCATION_KIND=internal \
        CAS_RELEASE_TRAIN_STAGE=pipeline CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" \
        CAS_RELEASE_TRAIN_BLOCKER_STAGES="${CAS_RELEASE_TRAIN_BLOCKER_STAGES:-none}" \
            "$0" "$version" "$worktree" --pipeline
    fi
}
