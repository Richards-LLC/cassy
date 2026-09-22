#!/usr/bin/env bash

if [[ -z "${script_dir:-}" ]]; then
    script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
fi

if ! declare -F release_train_date_stamp >/dev/null 2>&1; then
    # post-publication is also a standalone stage and may not follow announce.
    # shellcheck disable=SC1091
    source "$script_dir/release-train.d/announce.sh"
fi

release_train_post_publication_workflow() {
    local landed tag repo gh tries poll i workflows row status conclusion
    landed="$(tr -d '[:space:]' <"$run_dir/landed-main.sha" 2>/dev/null || true)"
    [[ "$landed" =~ ^[0-9a-f]{40}$ ]] || {
        printf 'ERROR post-publication: %s/landed-main.sha is missing or invalid\n' "$run_dir" >&2
        return 1
    }
    tag="v$version"
    repo="${CAS_RELEASE_TRAIN_REPO:-Richards-LLC/cassy}"
    gh="${CAS_RELEASE_TRAIN_GH:-gh}"
    tries="${CAS_RELEASE_TRAIN_POST_PUBLICATION_TRIES:-40}"
    poll="${CAS_RELEASE_TRAIN_POST_PUBLICATION_POLL_SECS:-15}"
    [[ "$tries" =~ ^[0-9]+$ ]] || tries=40
    [[ "$poll" =~ ^[0-9]+$ ]] || poll=15
    command -v "$gh" >/dev/null 2>&1 || {
        printf 'ERROR post-publication: GitHub CLI %s is not executable\n' "$gh" >&2
        return 1
    }
    for ((i = 1; i <= tries; i++)); do
        workflows="$("$gh" run list -R "$repo" --workflow release.yml --branch "$tag" \
            --event push --limit 20 \
            --json databaseId,status,conclusion,headBranch,headSha,createdAt 2>&1 || true)"
        row="$(printf '%s' "$workflows" | jq -c --arg tag "$tag" --arg sha "$landed" \
            '[.[]? | select(.headBranch == $tag and .headSha == $sha)] | sort_by(.createdAt) | first // empty' \
            2>/dev/null || true)"
        if [[ -n "$row" ]]; then
            status="$(printf '%s' "$row" | jq -r '.status // empty')"
            conclusion="$(printf '%s' "$row" | jq -r '.conclusion // empty')"
            if [[ "$status" == completed && "$conclusion" == success ]]; then
                printf '%s\n' "$row" >"$run_dir/release-workflow.json"
                return 0
            fi
            if [[ "$status" == completed && "$conclusion" != success ]]; then
                printf 'ERROR post-publication: Release workflow for %s/%s completed with %s\n' \
                    "$tag" "$landed" "${conclusion:-unknown}" >&2
                return 1
            fi
        fi
        if ((i < tries)); then
            sleep "$poll"
        fi
    done
    printf 'ERROR post-publication: no successful Release workflow for %s at %s after %s checks\n' \
        "$tag" "$landed" "$tries" >&2
    return 1
}

release_train_post_publication() {
    local gh repo tag draft published_cmd latency_cmd draft_args tmp
    gh="${CAS_RELEASE_TRAIN_GH:-gh}"
    repo="${CAS_RELEASE_TRAIN_REPO:-Richards-LLC/cassy}"
    tag="v$version"
    release_train_post_publication_workflow || return 1

    published_cmd="${CAS_RELEASE_TRAIN_PUBLISHED_RECEIPT_CMD:-$script_dir/release-published-receipt.sh}"
    latency_cmd="${CAS_RELEASE_TRAIN_LATENCY_RECEIPT_CMD:-$script_dir/release-latency-receipt.sh}"
    [[ -x "$published_cmd" ]] || {
        printf 'ERROR post-publication: published receipt command is not executable: %s\n' "$published_cmd" >&2
        return 1
    }
    [[ -x "$latency_cmd" ]] || {
        printf 'ERROR post-publication: latency receipt command is not executable: %s\n' "$latency_cmd" >&2
        return 1
    }

    draft=""
    if declare -F release_train_announce_draft_path >/dev/null 2>&1; then
        draft="$(release_train_announce_draft_path)"
    else
        draft="$worktree/docs/release-notes/$(release_train_date_stamp)-v${version}-slack.md"
    fi
    draft_args=("$tag")
    if [[ -f "$draft" ]] && grep -qE '\{\{(LINUX|MACOS)_SHA256\}\}' "$draft"; then
        draft_args+=(--write-draft "$draft")
    fi
    tmp="$run_dir/release-published.receipt.tmp"
    if ! GH_BIN="$gh" RELEASE_REPO="$repo" "$published_cmd" "${draft_args[@]}" >"$tmp"; then
        rm -f "$tmp"
        printf 'ERROR post-publication: published receipt command failed for %s\n' "$tag" >&2
        return 1
    fi
    mv "$tmp" "$run_dir/release-published.receipt"

    tmp="$run_dir/release-latency.receipt.tmp"
    if ! GH_BIN="$gh" RELEASE_REPO="$repo" CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" \
        "$latency_cmd" "$tag" --run-dir "$run_dir" >"$tmp"; then
        rm -f "$tmp"
        printf 'ERROR post-publication: latency receipt command failed for %s\n' "$tag" >&2
        return 1
    fi
    mv "$tmp" "$run_dir/release-latency.receipt"
}

cut_stage_post_publication() {
    if cut_has_external_stage post-publication; then
        cut_run_external_stage post-publication
        return
    fi
    release_train_post_publication
    local receipt
    for receipt in release-workflow.json release-published.receipt release-latency.receipt; do
        [[ -s "$run_dir/$receipt" ]] || {
            printf 'missing post-publication receipt %s/%s\n' "$run_dir" "$receipt" >&2
            return 1
        }
    done
}
