#!/usr/bin/env bash

cut_stage_publish() {
    if cut_has_external_stage publish; then
        cut_run_external_stage publish
    else
        local landed
        landed="$(cat "$run_dir/landed-main.sha" 2>/dev/null | tr -d '[:space:]' || true)"
        [[ -n "$landed" ]] || {
            printf 'pipeline did not record landed-main.sha\n' >&2
            return 1
        }
        "$0" "$version" "$worktree" --publish "$landed"
    fi
}
