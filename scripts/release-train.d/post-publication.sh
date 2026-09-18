#!/usr/bin/env bash

cut_stage_post_publication() {
    if cut_has_external_stage post-publication; then
        cut_run_external_stage post-publication
        return
    fi
    local receipt
    for receipt in release-workflow.json release-published.receipt release-latency.receipt; do
        [[ -s "$run_dir/$receipt" ]] || {
            printf 'missing post-publication receipt %s/%s\n' "$run_dir" "$receipt" >&2
            return 1
        }
    done
}
