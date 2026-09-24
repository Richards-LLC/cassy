#!/usr/bin/env bash
# Cheap, side-effect-bounded checks for --cut. No build or detached process
# starts until every check in this file has passed.

if ! declare -F release_train_date_stamp >/dev/null 2>&1; then
    release_train_date_stamp() {
        local date_stamp="${CAS_RELEASE_TRAIN_DATE:-}" started_at
        if [[ -z "$date_stamp" && -s "${run_dir:-}/run.env" ]]; then
            started_at="$(sed -n 's/^started_at=//p' "$run_dir/run.env" | head -n1 || true)"
            date_stamp="${started_at%%T*}"
        fi
        [[ "$date_stamp" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || date_stamp="$(date -u +%F)"
        printf '%s\n' "$date_stamp"
    }
fi

if ! declare -F release_portable_stat_device >/dev/null 2>&1; then
    # shellcheck source=scripts/release-portable.sh
    source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/release-portable.sh"
fi

if ! declare -F release_train_receipts_unmerged_records >/dev/null 2>&1; then
    # shellcheck disable=SC1091
    source "$script_dir/release-train.d/receipts-common.sh"
fi

cut_preflight_block() {
    local name="$1" detail="$2"
    printf 'BLOCKER %s: %s\n' "$name" "$detail" >&2
    printf 'receipt: %s\n' "$(cut_stage_file preflight)" >&2
    printf 'resume: %q %q %q --cut --resume\n' "$0" "$version" "$worktree" >&2
    return 1
}

cut_preflight_merge_queue_query() {
    printf '%s\n' 'query { repository(owner: "Richards-LLC", name: "cassy") { mergeQueue(branch: "main") { entries(first: 100) { nodes { pullRequest { number title headRefName } } } } } }'
}

cut_preflight_check_competing_release() {
    local gh="${CAS_RELEASE_TRAIN_GH:-gh}" prs queue query competing
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
    query="$(cut_preflight_merge_queue_query)"
    if ! queue="$("$gh" api graphql -f query="$query" 2>/dev/null)"; then
        cut_preflight_block competing-release "could not inspect the merge queue with $gh"
        return $?
    fi
    if ! printf '%s' "$queue" | jq -e \
        '.data.repository.mergeQueue.entries.nodes | type == "array"' >/dev/null 2>&1; then
        cut_preflight_block competing-release "merge queue response did not contain repository.mergeQueue.entries.nodes"
        return $?
    fi
    competing="$(printf '%s' "$queue" | jq -r \
        '.data.repository.mergeQueue.entries.nodes[]?.pullRequest
         | [(.title // ""), (.headRefName // "")] | join(" ")')"
    if printf '%s' "$competing" | grep -Eiq "release[ /_-]*${version}|v${version}"; then
        cut_preflight_block competing-release "a release pull request is already in the merge queue"
        return $?
    fi
}

cut_preflight_env_value() {
    local key="$1" env_file="${CAS_RELEASE_ENV_FILE:-$HOME/.cas/release.env}" line value
    [[ -r "$env_file" ]] || return 1
    line="$(grep -E "^(export )?${key}=" "$env_file" | head -n1 || true)"
    [[ -n "$line" ]] || return 1
    value="${line#*=}"
    value="${value#\"}"
    value="${value%\"}"
    value="${value#\'}"
    value="${value%\'}"
    printf '%s\n' "$value"
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
    local scratch="${CAS_RELEASE_GATE_HOME_DIR:-}" configured
    local probe archive required available previous scratch_parent checkout_device scratch_device
    if [[ -z "$scratch" ]]; then
        configured="$(cut_preflight_env_value CAS_RELEASE_GATE_HOME_DIR 2>/dev/null || true)"
        scratch="${configured:-$(release_portable_default_scratch_base)}"
    fi
    probe="$scratch/.release-train-write.$$"
    mkdir -p "$scratch" 2>/dev/null || {
        cut_preflight_block scratch-space "scratch base $scratch is not writable"
        return $?
    }
    : >"$probe" 2>/dev/null || {
        cut_preflight_block scratch-space "scratch base $scratch is not writable"
        return $?
    }
    rm -f "$probe"
    scratch_parent="$(dirname "$scratch")"
    checkout_device="${CAS_RELEASE_TRAIN_CHECKOUT_DEVICE:-$(release_portable_stat_device "$worktree" || true)}"
    scratch_device="${CAS_RELEASE_TRAIN_SCRATCH_DEVICE:-$(release_portable_stat_device "$scratch_parent" || true)}"
    if [[ -z "$checkout_device" || -z "$scratch_device" || "$checkout_device" != "$scratch_device" ]]; then
        cut_preflight_block scratch-space \
            "filesystem boundary: checkout device=${checkout_device:-unknown} scratch-parent device=${scratch_device:-unknown}"
        return $?
    fi
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
    available="$(df -Pk "$scratch_parent" 2>/dev/null | awk 'NR == 2 { print $4 * 1024; exit }')"
    if [[ "$required" -gt 0 && (! "$available" =~ ^[0-9]+$ || "$available" -lt "$required") ]]; then
        cut_preflight_block scratch-space \
            "scratch base $scratch has ${available:-unknown} bytes free; need at least $required"
        return $?
    fi
}

cut_preflight_check_env() {
    local env_file="${CAS_RELEASE_ENV_FILE:-$HOME/.cas/release.env}" names configured
    [[ -r "$env_file" ]] || {
        cut_preflight_block release-env "release env file $env_file is not readable"
        return $?
    }
    names="$(grep -oE '^(export )?[A-Z_]+=' "$env_file" | sed 's/=$//; s/^export //' | tr '\n' ' ')"
    if [[ -z "${CAS_RELEASE_GATE_HOME_DIR:-}" ]]; then
        configured="$(cut_preflight_env_value CAS_RELEASE_GATE_HOME_DIR 2>/dev/null || true)"
        [[ -z "$configured" ]] || export CAS_RELEASE_GATE_HOME_DIR="$configured"
    fi
    printf 'release env names: %s\n' "$names"
}

cut_preflight_check_zig() {
    local zig="${ZIG:-}" main_checkout zig_dir
    if [[ -n "$zig" && -x "$zig" ]]; then
        export ZIG="$zig"
        return 0
    fi
    if [[ -d "$worktree/.context/zig" && -x "$worktree/.context/zig/zig" ]]; then
        export ZIG="$worktree/.context/zig/zig"
        return 0
    fi
    main_checkout="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null \
        | sed 's#/\.git$##')"
    zig_dir="$worktree/.context/zig"
    if [[ -d "$main_checkout/.context/zig" && -x "$main_checkout/.context/zig/zig" ]]; then
        mkdir -p "$worktree/.context"
        if [[ -L "$zig_dir" ]]; then
            rm -f "$zig_dir"
        elif [[ -e "$zig_dir" && ! -d "$zig_dir" ]]; then
            cut_preflight_block zig "$zig_dir exists but is not a Zig toolchain directory"
            return $?
        fi
        if [[ ! -e "$zig_dir" ]]; then
            ln -s "$main_checkout/.context/zig" "$zig_dir"
        fi
        if [[ -d "$zig_dir" && -x "$zig_dir/zig" ]]; then
            export ZIG="$zig_dir/zig"
            return 0
        fi
        cut_preflight_block zig "could not link the main checkout Zig toolchain directory into $zig_dir"
        return $?
    fi
    if command -v zig >/dev/null 2>&1; then
        export ZIG="$(command -v zig)"
        return 0
    fi
    cut_preflight_block zig "zig is not resolvable from ZIG, the release worktree, or the main checkout"
    return $?
}

# cas-fed5: name every host tool the gate and publish stages need, before
# anything is built or detached. A macOS cut used to discover them one failed
# stage at a time (setsid, GNU stat, cargo-nextest off PATH, GNU objdump).
# Runs after the Zig check, so ZIG is resolved for the ISA self-test compiler.
cut_preflight_check_toolchain() {
    [[ "${CAS_RELEASE_TRAIN_PREFLIGHT_SKIP_TOOLCHAIN:-}" == 1 ]] && return 0
    local -a missing=()
    local tool listing
    for tool in cargo cargo-nextest cargo-zigbuild git jq python3; do
        command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
    done
    release_portable_setsid_prefix 2>/dev/null \
        || missing+=("setsid, or perl/python3 for its fallback")
    release_portable_stat_device "$worktree" >/dev/null \
        || missing+=("stat that reports a device number (GNU stat -c %d or BSD stat -f %d)")
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 \
        || missing+=("sha256sum or shasum")
    release_portable_gnu_objdump >/dev/null \
        || missing+=("GNU objdump for the x86_64 ISA audit (macOS: brew install binutils)")
    release_portable_x86_64_linux_cc \
        || missing+=("an x86_64 Linux C compiler for the ISA self-test (set CC, or provide zig)")
    if command -v rustup >/dev/null 2>&1; then
        listing="$(rustup target list --installed 2>/dev/null || true)"
        grep -qx 'x86_64-unknown-linux-gnu' <<<"$listing" \
            || missing+=("Rust target x86_64-unknown-linux-gnu (rustup target add x86_64-unknown-linux-gnu)")
    fi
    ((${#missing[@]} == 0)) && return 0
    local joined
    joined="$(printf '%s; ' "${missing[@]}")"
    cut_preflight_block toolchain "missing on this host: ${joined%; }"
}

cut_preflight_check_changelog() {
    local changelog="$worktree/CHANGELOG.md" date_stamp
    date_stamp="$(release_train_date_stamp)"
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

# The release PR is docs-only, so Docs Lint lints CHANGELOG.md in full; debt
# that entered alongside code would fail it there. Catch it before the PR.
# A host without Node warns rather than blocks: Docs Lint stays the backstop.
cut_preflight_check_changelog_lint() {
    local log="$run_dir/preflight-changelog-lint.log" status=0
    mkdir -p "$run_dir"
    "$script_dir/check-changelog-lint.sh" "$worktree" >"$log" 2>&1 || status=$?
    case "$status" in
        0) return 0 ;;
        2)
            printf 'warning: CHANGELOG.md lint skipped: %s\n' "$(tail -n 1 "$log")" >&2
            return 0
            ;;
        *)
            cat "$log" >&2
            cut_preflight_block changelog-lint \
                "CHANGELOG.md fails the Docs Lint markdownlint policy, so the docs-only release PR would fail Docs Lint; fix the findings in $log, commit, then resume"
            return $?
            ;;
    esac
}

cut_preflight_check_draft() {
    local date_stamp draft lint_dir lint_output
    date_stamp="$(release_train_date_stamp)"
    draft="$worktree/docs/release-notes/${date_stamp}-v${version}-slack.md"
    [[ -r "$draft" ]] || {
        cut_preflight_block release-draft "missing readable draft $draft"
        return $?
    }
    lint_dir="$run_dir/preflight-announce-bodies"
    mkdir -p "$lint_dir"
    if ! lint_output="$(python3 "$script_dir/release-train-announce.py" --validate "$draft" "$lint_dir" 2>&1)"; then
        printf '%s\n' "$lint_output" >"$run_dir/preflight-announce.log"
        printf '%s\n' "$lint_output" >&2
        cut_preflight_block release-draft \
            "announce lint failed for $draft; inspect $run_dir/preflight-announce.log"
        return $?
    fi
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

cut_preflight_check_receipts() {
    local record commit branch
    while IFS=$'\t' read -r record commit branch; do
        [[ -n "$record" ]] || continue
        printf 'preflight warning: unmerged prior receipts commit %s from %s; --prep will carry it forward\n' \
            "$commit" "${branch:-recorded receipt}"
    done < <(release_train_receipts_unmerged_records)
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
    cut_preflight_check_env || return 1
    cut_preflight_check_scratch || return 1
    cut_preflight_check_zig || return 1
    cut_preflight_check_toolchain || return 1
    cut_preflight_check_changelog || return 1
    cut_preflight_check_changelog_lint || return 1
    cut_preflight_check_draft || return 1
    cut_preflight_check_integration || return 1
    cut_preflight_check_receipts
    printf 'preflight passed version=%s worktree=%s\n' "$version" "$worktree"
}
