#!/usr/bin/env bash

cut_stage_pr_body() {
    if cut_has_external_stage pr-body; then
        cut_run_external_stage pr-body
        return
    fi
    local changelog="$worktree/CHANGELOG.md" output="$run_dir/pr-body.md"
    # Literal prefix matches, not dynamic regexes (cas-fed5): "\[" inside an
    # awk string is an escape only some awks keep (mawk did; macOS's BWK awk
    # and gawk turn "^## \[3.29.0\]" into a bracket expression), and the dots
    # in a version are regex wildcards anyway. index() is POSIX in every awk.
    awk -v heading="## [$version]" '
        index($0, heading) == 1 { found = 1; print; next }
        found && index($0, "## [") == 1 { exit }
        found { print }
    ' "$changelog" >"$output"
    [[ -s "$output" ]] || {
        printf 'could not derive CHANGELOG section for %s\n' "$version" >&2
        return 1
    }
}
