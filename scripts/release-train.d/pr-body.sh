#!/usr/bin/env bash

cut_stage_pr_body() {
    if cut_has_external_stage pr-body; then
        cut_run_external_stage pr-body
        return
    fi
    local changelog="$worktree/CHANGELOG.md" output="$run_dir/pr-body.md"
    awk -v version="$version" '
        $0 ~ "^## \[" version "\]" { found = 1 }
        found && $0 ~ "^## \[" && $0 !~ "^## \[" version "\]" { exit }
        found { print }
    ' "$changelog" >"$output"
    [[ -s "$output" ]] || {
        printf 'could not derive CHANGELOG section for %s\n' "$version" >&2
        return 1
    }
}
