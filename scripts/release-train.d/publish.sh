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
        CAS_RELEASE_TRAIN_INVOCATION_KIND=internal \
        CAS_RELEASE_TRAIN_STAGE=publish CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" \
        CAS_RELEASE_TRAIN_BLOCKER_STAGES="${CAS_RELEASE_TRAIN_BLOCKER_STAGES:-none}" \
            "$0" "$version" "$worktree" --publish "$landed"
    fi
}
