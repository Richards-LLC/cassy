#!/usr/bin/env bash

# Release-train prep owns the draft that enters the release commit. The
# --cut dispatcher sources this file, so its public seam is release_train_prep.

release_train_draft_path() {
    local date_part="${CAS_RELEASE_TRAIN_DATE:-$(date -u +%F)}"
    printf '%s\n' "${CAS_RELEASE_TRAIN_DRAFT:-$worktree/docs/release-notes/${date_part}-v${version}-slack.md}"
}

release_train_previous_draft() {
    local draft="$1"
    find "$worktree/docs/release-notes" -maxdepth 1 -type f \
        -name '*-v*-slack.md' ! -path "$draft" -print 2>/dev/null \
        | sort -V | tail -n1
}

release_train_carry_previous_posted() {
    local draft="$1" prior posted prior_version temp
    if grep -qE '^## (Prior release receipt carried forward|v[^ ]+ POSTED)' "$draft"; then
        return 0
    fi
    prior="$(release_train_previous_draft "$draft")"
    [[ -n "$prior" && -f "$prior" ]] || return 0
    posted="$(awk '/^## .* POSTED/{found=1} found{print}' "$prior")"
    [[ -n "$posted" ]] || return 0
    prior_version="$(printf '%s\n' "$posted" | sed -n 's/^## \(v[^ ]*\) POSTED.*/\1/p' | head -n1)"
    [[ -n "$prior_version" ]] || prior_version="$(basename "$prior" | sed -n 's/.*-\(v[^-]*\)-slack\.md/\1/p')"
    temp="$(mktemp "$worktree/.release-draft.XXXXXX")"
    {
        cat "$draft"
        printf '\n## Prior release receipt carried forward (%s; not v%s POSTED evidence)\n\n' \
            "$prior_version" "$version"
        printf '%s\n' "$posted"
    } >"$temp"
    mv "$temp" "$draft"
}

release_train_prep() {
    local draft commit_sha
    draft="$(release_train_draft_path)"
    if [[ ! -f "$draft" ]]; then
        printf 'ERROR prep draft: required draft is missing: %s\n' "$draft" >&2
        printf '  → create the draft, then rerun --prep\n' >&2
        return 1
    fi
    release_train_carry_previous_posted "$draft"
    git -C "$worktree" add -- "$draft"
    for prep_input in CHANGELOG.md Cargo.lock Cargo.toml cas-cli/Cargo.toml \
        crates/cas-core/Cargo.toml crates/cas-mcp/Cargo.toml crates/cas-search/Cargo.toml \
        crates/cas-store/Cargo.toml crates/cas-types/Cargo.toml; do
        [[ -e "$worktree/$prep_input" ]] && git -C "$worktree" add -- "$prep_input"
    done
    if ! git -C "$worktree" diff --cached --quiet; then
        git -C "$worktree" -c core.hooksPath=/dev/null commit -m "release: prepare v$version" >/dev/null
    fi
    commit_sha="$(git -C "$worktree" rev-parse HEAD)"
    printf 'prep complete · draft=%s · commit=%s\n' "$draft" "$commit_sha"
}

stage_prep() {
    release_train_prep "$@"
}

cut_stage_prep() {
    if cut_has_external_stage prep; then
        cut_run_external_stage prep
    else
        release_train_prep "$@"
    fi
}
