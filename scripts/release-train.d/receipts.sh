#!/usr/bin/env bash

if [[ -z "${script_dir:-}" ]]; then
    script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fi

if ! declare -F release_train_announce_draft_path >/dev/null 2>&1; then
    # --receipts is a standalone stage as well as a --cut stage.
    # shellcheck disable=SC1091
    source "$script_dir/release-train.d/announce.sh"
fi
if ! declare -F release_train_receipts_record_files >/dev/null 2>&1; then
    # shellcheck disable=SC1091
    source "$script_dir/release-train.d/receipts-common.sh"
fi

release_train_receipts() {
    local draft receipt landed branch report_dir artifact draft_rel commit_sha
    local receipt_temp candidate_subject
    local -a report_paths=()
    if [[ -s "$run_dir/receipts.commit" ]]; then
        commit_sha="$(release_train_receipts_record_field "$run_dir/receipts.commit" COMMIT_SHA)"
        if [[ "$commit_sha" =~ ^[0-9a-f]{40}$ ]] \
            && git -C "$worktree" cat-file -e "$commit_sha^{commit}" 2>/dev/null \
            && git -C "$worktree" merge-base --is-ancestor "$commit_sha" HEAD 2>/dev/null; then
            printf 'receipts complete · commit=%s · receipt=%s\n' "$commit_sha" "$run_dir/receipts.commit"
            return 0
        fi
        printf 'ERROR receipts: recorded commit is not present in the release branch: %s\n' \
            "$run_dir/receipts.commit" >&2
        return 1
    fi

    draft="$(release_train_announce_draft_path 2>/dev/null || printf '%s/docs/release-notes/%s-v%s-slack.md' \
        "$worktree" "${CAS_RELEASE_TRAIN_DATE:-$(date -u +%F)}" "$version")"
    receipt="$run_dir/announce.receipt"
    if [[ ! -s "$receipt" ]]; then
        printf 'ERROR receipts announcement: missing %s\n' "$receipt" >&2
        printf '  → run --announce successfully before --receipts\n' >&2
        return 1
    fi
    for key in POSTED_AT CHANNEL USER_TOP_LEVEL_ID USER_TOP_LEVEL_PERMALINK \
        USER_REPLY_ID USER_REPLY_PERMALINK DEV_TOP_LEVEL_ID DEV_TOP_LEVEL_PERMALINK \
        DEV_REPLY_ID DEV_REPLY_PERMALINK; do
        if ! release_train_announce_receipt_field "$key" | grep -q '[^[:space:]]'; then
            printf 'ERROR receipts announcement: missing %s in %s\n' "$key" "$receipt" >&2
            return 1
        fi
    done
    if [[ ! -f "$draft" ]]; then
        printf 'ERROR receipts draft: required draft is missing: %s\n' "$draft" >&2
        return 1
    fi
    landed="$(tr -d '[:space:]' <"$run_dir/landed-main.sha" 2>/dev/null || true)"
    if [[ ! "$landed" =~ ^[0-9a-f]{40}$ ]]; then
        printf 'ERROR receipts base: %s/landed-main.sha is missing or invalid\n' "$run_dir" >&2
        printf '  → complete --pipeline, then rerun --receipts\n' >&2
        return 1
    fi
    branch="$(git -C "$worktree" branch --show-current)"
    if [[ -z "$branch" || "$branch" != release/* && "$branch" != receipts/* ]]; then
        printf 'ERROR receipts branch: release evidence must be committed on a release/ or receipts/ branch (got %s)\n' \
            "${branch:-detached}" >&2
        return 1
    fi
    if ! git -C "$worktree" remote get-url origin >/dev/null 2>&1; then
        printf 'ERROR receipts branch: origin is required to publish the release-branch commit\n' >&2
        return 1
    fi
    if ! grep -q '^## POSTED$' "$draft"; then
        if ! release_train_announce_append_posted "$draft"; then
            printf 'ERROR receipts draft: could not append POSTED block to %s\n' "$draft" >&2
            return 1
        fi
    fi
    report_dir="$worktree/docs/release-reports"
    for artifact in "$report_dir/v$version"*; do
        [[ -f "$artifact" ]] || continue
        report_paths+=("${artifact#"$worktree"/}")
    done
    if ((${#report_paths[@]} == 0)); then
        printf 'ERROR receipts report: no docs/release-reports/v%s* files found\n' "$version" >&2
        return 1
    fi
    draft_rel="${draft#"$worktree"/}"
    git -C "$worktree" add -- "$draft_rel" "${report_paths[@]}"
    if git -C "$worktree" diff --cached --quiet; then
        candidate_subject="$(git -C "$worktree" log -1 --format=%s -- "$draft_rel")"
        if [[ "$candidate_subject" != "docs(release): record v$version receipts and report" ]]; then
            printf 'ERROR receipts: no release evidence changes are staged for v%s\n' "$version" >&2
            return 1
        fi
        commit_sha="$(git -C "$worktree" rev-parse HEAD)"
    else
        git -C "$worktree" -c core.hooksPath=/dev/null commit \
            -m "docs(release): record v$version receipts and report" >/dev/null
        commit_sha="$(git -C "$worktree" rev-parse HEAD)"
    fi
    if ! git -C "$worktree" push -q origin "HEAD:refs/heads/$branch"; then
        printf 'ERROR receipts branch: could not push %s to origin\n' "$commit_sha" >&2
        return 1
    fi
    receipt_temp="$run_dir/receipts.commit.tmp.$$"
    cat >"$receipt_temp" <<EOF
COMMIT_SHA=$commit_sha
BRANCH=$branch
BASE_SHA=$landed
EOF
    mv "$receipt_temp" "$run_dir/receipts.commit"
    printf 'receipts commit %s · branch=%s · base=%s\n' "$commit_sha" "$branch" "$landed"
}

stage_receipts() {
    release_train_receipts "$@"
}

cut_stage_receipts() {
    if cut_has_external_stage receipts; then
        cut_run_external_stage receipts
    else
        release_train_receipts "$@"
    fi
}
