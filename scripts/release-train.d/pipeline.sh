#!/usr/bin/env bash

cut_stage_pipeline() {
    if cut_has_external_stage pipeline; then
        cut_run_external_stage pipeline
    else
        "$0" "$version" "$worktree" --pipeline
    fi
}
