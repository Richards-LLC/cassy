#!/usr/bin/env bash
# Cheap, side-effect-bounded checks for --cut. No build or detached process
# starts until every check in this file has passed.

cut_preflight_block() {
    local name="$1" detail="$2"
    printf 'BLOCKER %s: %s\n' "$name" "$detail" >&2
    printf 'receipt: %s\n' "$(cut_stage_file preflight)" >&2
    printf 'resume: %q %q %q --cut --resume\n' "$0" "$version" "$worktree" >&2
    return 1
}

cut_preflight_check_competing_release() {
    local gh="${CAS_RELEASE_TRAIN_GH:-gh}" prs queue
    [[ "${CAS_RELEASE_TRAIN_PREFLIGHT_SKIP_COMPETING:-}" == 1 ]] && return 0
    if ! command -v "$gh" >/dev/null 2>&1; then
        cut_preflight_block competing-release "GitHub CLI $gh is not available"
        return $?
    fi
    if ! prs="$("$gh" pr list -R "${CAS_RELEASE_TRAIN_REPO:-Richards-LLC/cassy}" \
        --state open --json number,title,headRefName 2>/dev/null)"; then
        cut_preflight_block competing-release "could not inspect open release PRs with $gh"
        return $?
    fi
    if command -v jq >/dev/null 2>&1 && printf '%s' "$prs" | jq -e --arg v "$version" '
        any(.[]; ((.title // "") + " " + (.headRefName // "")) |
        test("release[ /_-]*" + $v + "|v" + $v; "i"))
    ' >/dev/null 2>&1; then
        cut_preflight_block competing-release "an open release PR already targets $version"
        return $?
    fi
    if ! queue="$("$gh" api graphql -f query='query { repository(owner: "Richards-LLC", name: "cassy") { mergeQueue(branch: \"main\") { entries(first: 100) { nodes { pullRequest { number title headRefName } } } } } }' 2>/dev/null)"; then
        cut_preflight_block competing-release "could not inspect the merge queue with $gh"
        return $?
    fi
    if printf '%s' "$queue" | grep -Eiq "release[ /_-]*${version}|v${version}"; then
        cut_preflight_block competing-release "a release pull request is already in the merge queue"
        return $?
    fi
}

cut_preflight_check_tag() {
    local tag="v$version" remote_tags
    if git -C "$worktree" show-ref --tags --verify --quiet "refs/tags/$tag"; then
        cut_preflight_block version-tag "tag $tag already exists locally"
        return $?
    fi
    if git -C "$worktree" remote get-url origin >/dev/null 2>&1; then
        if ! remote_tags="$(git -C "$worktree" ls-remote --tags origin "refs/tags/$tag" 2>/dev/null)"; then
            cut_preflight_block version-tag "could not inspect origin for tag $tag"
            return $?
        fi
    else
        remote_tags=''
    fi
    if [[ -n "$remote_tags" ]]; then
        cut_preflight_block version-tag "tag $tag already exists on origin"
        return $?
    fi
}

cut_preflight_check_scratch() {
    local scratch="${CAS_RELEASE_GATE_HOME_DIR:-/var/tmp/cas-release-gate}"
    local probe="$scratch/.release-train-write.$$" archive required available previous
    mkdir -p "$scratch" 2>/dev/null || {
        cut_preflight_block scratch-space "scratch base $scratch is not writable"
        return $?
    }
    : >"$probe" 2>/dev/null || {
        cut_preflight_block scratch-space "scratch base $scratch is not writable"
        return $?
    }
    rm -f "$probe"
    archive="${CAS_RELEASE_TRAIN_LAST_ARCHIVE_SIZE:-}"
    if [[ -z "$archive" && -s "$scratch/archive-size-bytes" ]]; then
        archive="$(tr -d '[:space:]' <"$scratch/archive-size-bytes")"
    fi
    if [[ -z "$archive" && -s "$run_dir/archive-size-bytes" ]]; then
        archive="$(tr -d '[:space:]' <"$run_dir/archive-size-bytes")"
    fi
    if [[ -z "$archive" ]]; then
        previous="$(find "$artifacts_root" -type f -name archive-size-bytes -print 2>/dev/null | sort | tail -1 || true)"
        if [[ -n "$previous" && -s "$previous" ]]; then
            archive="$(tr -d '[:space:]' <"$previous")"
        fi
    fi
    [[ "$archive" =~ ^[0-9]+$ ]] || archive=0
    required=$((archive * 2))
    available="$(df -Pk "$scratch" 2>/dev/null | awk 'NR == 2 { print $4 * 1024; exit }')"
    if [[ "$required" -gt 0 && (! "$available" =~ ^[0-9]+$ || "$available" -lt "$required") ]]; then
        cut_preflight_block scratch-space \
            "scratch base $scratch has ${available:-unknown} bytes free; need at least $required"
        return $?
    fi
}

cut_preflight_check_env() {
    local env_file="${CAS_RELEASE_ENV_FILE:-$HOME/.cas/release.env}" names
    [[ -r "$env_file" ]] || {
        cut_preflight_block release-env "release env file $env_file is not readable"
        return $?
    }
    names="$(grep -oE '^(export )?[A-Z_]+=' "$env_file" | sed 's/=$//; s/^export //' | tr '\n' ' ')"
    printf 'release env names: %s\n' "$names"
}

cut_preflight_check_zig() {
    local zig="${ZIG:-}" main_checkout
    if [[ -n "$zig" && -x "$zig" ]]; then
        export ZIG="$zig"
        return 0
    fi
    if [[ -x "$worktree/.context/zig/zig" ]]; then
        export ZIG="$worktree/.context/zig/zig"
        return 0
    fi
    main_checkout="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null \
        | sed 's#/\.git$##')"
    if [[ -x "$main_checkout/.context/zig/zig" ]]; then
        mkdir -p "$worktree/.context"
        ln -s "$main_checkout/.context/zig/zig" "$worktree/.context/zig"
        export ZIG="$worktree/.context/zig/zig"
        return 0
    fi
    if command -v zig >/dev/null 2>&1; then
        export ZIG="$(command -v zig)"
        return 0
    fi
    cut_preflight_block zig "zig is not resolvable from ZIG, the release worktree, or the main checkout"
    return $?
}

cut_preflight_check_changelog() {
    local changelog="$worktree/CHANGELOG.md" date_stamp="${CAS_RELEASE_TRAIN_DATE:-$(date -u +%F)}"
    if grep -Eq "^## \[$version\]( - [0-9]{4}-[0-9]{2}-[0-9]{2})?$" "$changelog"; then
        return 0
    fi
    if ! grep -Eq '^## \[Unreleased\]' "$changelog"; then
        cut_preflight_block changelog-heading "CHANGELOG.md has no Unreleased section and no $version heading"
        return $?
    fi
    local tmp="$changelog.cut.$$"
    awk -v version="$version" -v date_stamp="$date_stamp" '
        BEGIN { inserted = 0 }
        /^## \[Unreleased\]/ && !inserted {
            print
            print ""
            print "## [" version "] - " date_stamp
            print ""
            print "- Release entries are pending; fill this section before resuming."
            inserted = 1
            next
        }
        { print }
    ' "$changelog" >"$tmp"
    mv "$tmp" "$changelog"
    cut_preflight_block changelog-heading \
        "created CHANGELOG.md heading for $version; fill the section, commit it, then resume"
    return $?
}

cut_preflight_check_draft() {
    local date_stamp="${CAS_RELEASE_TRAIN_DATE:-$(date -u +%F)}"
    local draft="$worktree/docs/release-notes/${date_stamp}-v${version}-slack.md"
    [[ -r "$draft" ]] || {
        cut_preflight_block release-draft "missing readable draft $draft"
        return $?
    }
}

cut_preflight_check_integration() {
    local common receipt origin_main receipt_base
    common="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null \
        | sed 's#/\.git$##')"
    receipt="$common/.cas/merge-sweeps/integration.json"
    [[ -r "$receipt" ]] || {
        cut_preflight_block integration-receipt "missing passing integration receipt $receipt"
        return $?
    }
    origin_main="$(git -C "$worktree" rev-parse --verify refs/remotes/origin/main 2>/dev/null || true)"
    [[ -n "$origin_main" ]] || {
        cut_preflight_block integration-base-stale "origin/main is not available for comparison"
        return $?
    }
    receipt_base="$(jq -r '.base // empty' "$receipt" 2>/dev/null || true)"
    [[ "$receipt_base" =~ ^[0-9a-f]{40}$ ]] || {
        cut_preflight_block integration-receipt "integration receipt has no valid base SHA"
        return $?
    }
    if [[ "$origin_main" != "$receipt_base" ]]; then
        printf 'preflight: origin/main=%s differs from receipt base=%s; assemble will invoke bounded self-heal\n' \
            "$origin_main" "$receipt_base"
    fi
}

cut_stage_preflight() {
    local branch
    branch="$(git -C "$worktree" branch --show-current)"
    [[ -n "$branch" && "$branch" == release/* ]] || {
        cut_preflight_block release-branch \
            "release worktree must be on a release/ branch (got ${branch:-detached})"
        return $?
    }
    [[ -z "$(git -C "$worktree" status --porcelain)" ]] || {
        cut_preflight_block clean-worktree "release worktree has uncommitted changes"
        return $?
    }
    cut_preflight_check_competing_release || return 1
    cut_preflight_check_tag || return 1
    cut_preflight_check_scratch || return 1
    cut_preflight_check_env || return 1
    cut_preflight_check_zig || return 1
    cut_preflight_check_changelog || return 1
    cut_preflight_check_draft || return 1
    cut_preflight_check_integration || return 1
    printf 'preflight passed version=%s worktree=%s\n' "$version" "$worktree"
}
