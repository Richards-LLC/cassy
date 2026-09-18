#!/usr/bin/env bash

cut_stage_report() {
    if cut_has_external_stage report; then
        cut_run_external_stage report
    else
        CAS_RELEASE_TRAIN_INVOCATION_KIND=internal \
        CAS_RELEASE_TRAIN_STAGE=report CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" \
        CAS_RELEASE_TRAIN_BLOCKER_STAGES="${CAS_RELEASE_TRAIN_BLOCKER_STAGES:-none}" \
            "$0" "$version" "$worktree" --report
    fi
}
