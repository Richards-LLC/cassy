#!/usr/bin/env bash

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

release_train_announce_draft_path() {
    local date_part
    date_part="$(release_train_date_stamp)"
    printf '%s\n' "${CAS_RELEASE_TRAIN_DRAFT:-$worktree/docs/release-notes/${date_part}-v${version}-slack.md}"
}

release_train_announce_proxy_toml() {
    if [[ -n "${CAS_RELEASE_TRAIN_PROXY_TOML:-}" ]]; then
        printf '%s\n' "$CAS_RELEASE_TRAIN_PROXY_TOML"
    else
        git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null \
            | sed 's#/\.git$#/.cas/proxy.toml#'
    fi
}

release_train_announce_receipt_field() {
    local key="$1"
    sed -n "s/^${key}=//p" "$run_dir/announce.receipt" | head -n1
}

# The operator reads Eastern 12-hour times; announce.receipt keeps UTC ISO-8601.
release_train_announce_eastern_time() {
    python3 - "$1" <<'PY'
import sys
from datetime import datetime
from zoneinfo import ZoneInfo

value = sys.argv[1]
try:
    instant = datetime.fromisoformat(value.replace("Z", "+00:00"))
    local = instant.astimezone(ZoneInfo("America/New_York"))
except (ValueError, KeyError, OSError):
    print(value)
else:
    print(f"{local.day} {local:%B %Y} · {local.hour % 12 or 12}:{local:%M %p} ET")
PY
}

release_train_announce_posted_block() {
    local posted_at channel channel_id channel_display tick report_message report_link report_file
    posted_at="$(release_train_announce_eastern_time "$(release_train_announce_receipt_field POSTED_AT)")"
    channel="$(release_train_announce_receipt_field CHANNEL)"
    channel_id="$(release_train_announce_receipt_field CHANNEL_ID)"
    channel_display="#${channel}"
    [[ -n "$channel_id" ]] && channel_display="$channel_display ($channel_id)"
    tick="$(printf '\x60')"
    cat <<EOF
## POSTED

- **Posted at:** ${tick}${posted_at}${tick}
- **Channel:** ${tick}${channel_display}${tick}
- **User top-level:** ${tick}message_id=$(release_train_announce_receipt_field USER_TOP_LEVEL_ID)${tick} · ${tick}part_ids=$(release_train_announce_receipt_field USER_TOP_LEVEL_PART_IDS)${tick} · <$(release_train_announce_receipt_field USER_TOP_LEVEL_PERMALINK)>
- **User reply:** ${tick}message_id=$(release_train_announce_receipt_field USER_REPLY_ID)${tick} · ${tick}part_ids=$(release_train_announce_receipt_field USER_REPLY_PART_IDS)${tick} · <$(release_train_announce_receipt_field USER_REPLY_PERMALINK)>
- **Dev top-level:** ${tick}message_id=$(release_train_announce_receipt_field DEV_TOP_LEVEL_ID)${tick} · ${tick}part_ids=$(release_train_announce_receipt_field DEV_TOP_LEVEL_PART_IDS)${tick} · <$(release_train_announce_receipt_field DEV_TOP_LEVEL_PERMALINK)>
- **Dev reply:** ${tick}message_id=$(release_train_announce_receipt_field DEV_REPLY_ID)${tick} · ${tick}part_ids=$(release_train_announce_receipt_field DEV_REPLY_PART_IDS)${tick} · <$(release_train_announce_receipt_field DEV_REPLY_PERMALINK)>
EOF
    if [[ -s "$run_dir/release-report.receipt" ]]; then
        report_message="$(sed -n 's/^PDF_MESSAGE_ID=//p' "$run_dir/release-report.receipt" | head -n1)"
        report_link="$(sed -n 's/^PDF_MESSAGE_PERMALINK=//p' "$run_dir/release-report.receipt" | head -n1)"
        report_file="$(sed -n 's/^PDF_FILE_ID=//p' "$run_dir/release-report.receipt" | head -n1)"
        if [[ -z "$report_message" || -z "$report_file" || "$report_link" != https://* ]]; then
            printf 'ERROR report receipt: missing PDF upload message identity in %s/release-report.receipt\n' "$run_dir" >&2
            return 1
        fi
        printf '%s\n' "- **PDF upload:** ${tick}message_id=${report_message}${tick} · ${tick}file_id=${report_file}${tick} · <${report_link}>"
    fi
}

release_train_announce_append_posted() {
    local draft="$1" temp
    # Receipts are authoritative: refresh the block without reposting, including
    # split parts and a report upload that completed after --announce.
    temp="$(mktemp "$worktree/.release-draft-posted.XXXXXX")"
    if ! {
        awk '/^## POSTED$/ { skip = 1; next } skip && /^## / { skip = 0 } !skip { print }' "$draft" \
            | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}'
        printf '\n'
        release_train_announce_posted_block
    } >"$temp"; then
        rm -f "$temp"
        return 1
    fi
    mv "$temp" "$draft"
}

release_train_announcement_embargo_active() {
    local embargo="${CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO:-}"
    if [[ -z "${CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO+x}" && -s "$run_dir/announcement-embargo.txt" ]]; then
        embargo="$(cat "$run_dir/announcement-embargo.txt")"
    fi
    [[ -n "${embargo//[[:space:]]/}" ]]
}

release_train_announce() {
    local draft body_dir post_cmd receipt proxy_toml
    if release_train_announcement_embargo_active; then
        printf 'BLOCKER announce: explicit announcement embargo; runtime publication and install proof remain required\n' >&2
        return 1
    fi
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
            DEV_REPLY_ID DEV_REPLY_PERMALINK \
        USER_TOP_LEVEL_PART_IDS USER_REPLY_PART_IDS DEV_TOP_LEVEL_PART_IDS DEV_REPLY_PART_IDS; do
            if ! sed -n "s/^${key}=//p" "$receipt" | head -n1 | grep -q '[^[:space:]]'; then
                printf 'ERROR announce receipt: partial receipt at %s; refusing to retry writes\n' "$receipt" >&2
                return 1
            fi
        done
        if ! release_train_announce_append_posted "$draft"; then
            printf 'ERROR announce receipt: could not append POSTED block to %s\n' "$draft" >&2
            return 1
        fi
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
    proxy_toml="$(release_train_announce_proxy_toml)"
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
        DEV_REPLY_ID DEV_REPLY_PERMALINK \
        USER_TOP_LEVEL_PART_IDS USER_REPLY_PART_IDS DEV_TOP_LEVEL_PART_IDS DEV_REPLY_PART_IDS; do
        if ! sed -n "s/^${key}=//p" "$receipt" | head -n1 | grep -q '[^[:space:]]'; then
            printf 'ERROR announce receipt: missing %s in %s\n' "$key" "$receipt" >&2
            return 1
        fi
    done
    if ! release_train_announce_append_posted "$draft"; then
        printf 'ERROR announce receipt: could not append POSTED block to %s\n' "$draft" >&2
        return 1
    fi
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
