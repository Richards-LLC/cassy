#!/usr/bin/env bash

cut_stage_assemble() {
    if cut_has_external_stage assemble; then
        cut_run_external_stage assemble
    else
        python3 "$script_dir/release-integrate.py" "$worktree"
    fi
}
