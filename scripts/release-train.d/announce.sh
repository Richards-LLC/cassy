#!/usr/bin/env bash

release_train_announce_draft_path() {
    local date_part="${CAS_RELEASE_TRAIN_DATE:-$(date -u +%F)}"
    printf '%s\n' "${CAS_RELEASE_TRAIN_DRAFT:-$worktree/docs/release-notes/${date_part}-v${version}-slack.md}"
}

release_train_announce() {
    local draft body_dir post_cmd receipt proxy_toml
    draft="$(release_train_announce_draft_path)"
    receipt="$run_dir/announce.receipt"
    body_dir="$run_dir/announce-bodies"
    mkdir -p "$run_dir"
    if [[ ! -f "$draft" ]]; then
        printf 'ERROR announce draft: required draft is missing: %s\n' "$draft" >&2
        printf '  → run --prep with the release draft, then rerun --announce\n' >&2
        return 1
    fi
    if [[ -s "$receipt" ]]; then
        for key in POSTED_AT CHANNEL USER_TOP_LEVEL_ID USER_TOP_LEVEL_PERMALINK \
            USER_REPLY_ID USER_REPLY_PERMALINK DEV_TOP_LEVEL_ID DEV_TOP_LEVEL_PERMALINK \
            DEV_REPLY_ID DEV_REPLY_PERMALINK; do
            if ! sed -n "s/^${key}=//p" "$receipt" | head -n1 | grep -q '[^[:space:]]'; then
                printf 'ERROR announce receipt: partial receipt at %s; refusing to retry writes\n' "$receipt" >&2
                return 1
            fi
        done
        printf 'announce complete · receipt=%s\n' "$receipt"
        return 0
    fi
    mkdir -p "$body_dir"
    if ! python3 "$script_dir/release-train-announce.py" --validate "$draft" "$body_dir"; then
        printf 'ERROR announce lint failed: %s\n' "$draft" >&2
        printf '  → fix the four fenced bodies, then rerun --announce\n' >&2
        return 1
    fi
    post_cmd="${CAS_RELEASE_TRAIN_ANNOUNCE_POST_CMD:-}"
    proxy_toml="${CAS_RELEASE_TRAIN_PROXY_TOML:-}"
    if [[ -z "$proxy_toml" ]]; then
        proxy_toml="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null \
            | sed 's#/\.git$#/.cas/proxy.toml#')"
    fi
    if [[ -n "$post_cmd" ]]; then
        CAS_RELEASE_TRAIN_ANNOUNCE_BODY_DIR="$body_dir" \
        CAS_RELEASE_TRAIN_ANNOUNCE_RECEIPT="$receipt" \
        CAS_RELEASE_TRAIN_ANNOUNCE_DRAFT="$draft" \
        CAS_RELEASE_TRAIN_ANNOUNCE_VERSION="$version" \
        CAS_RELEASE_TRAIN_PROXY_TOML="$proxy_toml" \
            "$post_cmd" "$version" "$draft" "$receipt" "$body_dir"
    else
        CAS_RELEASE_TRAIN_PROXY_TOML="$proxy_toml" \
            python3 "$script_dir/release-train-announce.py" --post \
                "$version" "$draft" "$receipt" "$body_dir"
    fi
    if ! test -s "$receipt"; then
        printf 'ERROR announce receipt: adapter returned without %s\n' "$receipt" >&2
        printf '  → preserve any partial receipt and inspect the adapter before retrying\n' >&2
        return 1
    fi
    for key in POSTED_AT CHANNEL USER_TOP_LEVEL_ID USER_TOP_LEVEL_PERMALINK \
        USER_REPLY_ID USER_REPLY_PERMALINK DEV_TOP_LEVEL_ID DEV_TOP_LEVEL_PERMALINK \
        DEV_REPLY_ID DEV_REPLY_PERMALINK; do
        if ! sed -n "s/^${key}=//p" "$receipt" | head -n1 | grep -q '[^[:space:]]'; then
            printf 'ERROR announce receipt: missing %s in %s\n' "$key" "$receipt" >&2
            return 1
        fi
    done
    printf 'announce complete · receipt=%s\n' "$receipt"
}

stage_announce() {
    release_train_announce "$@"
}

cut_stage_announce() {
    if cut_has_external_stage announce; then
        cut_run_external_stage announce
    else
        release_train_announce "$@"
    fi
}
