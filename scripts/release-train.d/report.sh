#!/usr/bin/env bash

cut_stage_report() {
    if cut_has_external_stage report; then
        cut_run_external_stage report
    else
        "$0" "$version" "$worktree" --report
    fi
}
