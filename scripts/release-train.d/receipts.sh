#!/usr/bin/env bash

if ! declare -F release_train_announce_draft_path >/dev/null 2>&1; then
    # --receipts is a standalone stage as well as a --cut stage.
    # shellcheck disable=SC1091
    source "$script_dir/release-train.d/announce.sh"
fi

release_train_receipts_field() {
    local key="$1"
    sed -n "s/^${key}=//p" "$run_dir/announce.receipt" | head -n1
}

release_train_receipts_posted_block() {
    local posted_at channel channel_id channel_display tick
    posted_at="$(release_train_receipts_field POSTED_AT)"
    channel="$(release_train_receipts_field CHANNEL)"
    channel_id="$(release_train_receipts_field CHANNEL_ID)"
    channel_display="#${channel}"
    [[ -n "$channel_id" ]] && channel_display="$channel_display ($channel_id)"
    tick="$(printf '\x60')"
    cat <<EOF
## POSTED

- **Posted at (UTC):** ${tick}${posted_at}${tick}
- **Channel:** ${tick}${channel_display}${tick}
- **User top-level:** ${tick}message_id=$(release_train_receipts_field USER_TOP_LEVEL_ID)${tick} · $(release_train_receipts_field USER_TOP_LEVEL_PERMALINK)
- **User reply:** ${tick}message_id=$(release_train_receipts_field USER_REPLY_ID)${tick} · $(release_train_receipts_field USER_REPLY_PERMALINK)
- **Dev top-level:** ${tick}message_id=$(release_train_receipts_field DEV_TOP_LEVEL_ID)${tick} · $(release_train_receipts_field DEV_TOP_LEVEL_PERMALINK)
- **Dev reply:** ${tick}message_id=$(release_train_receipts_field DEV_REPLY_ID)${tick} · $(release_train_receipts_field DEV_REPLY_PERMALINK)
EOF
}

release_train_receipts_standard_body() {
    cat <<EOF
This PR carries the v$version release announcement receipt and report evidence.

- Base: $1
- Draft: POSTED receipt from announce.receipt
- Report: docs/release-reports/v$version*

The receipt and report files are committed together so the published docs share one reviewed change.
EOF
}

release_train_receipts_wait_mergeable() {
    local gh="$1" repo="$2" pr_number="$3" tries poll i view mergeable state
    tries="${CAS_RELEASE_TRAIN_RECEIPTS_MERGEABLE_TRIES:-60}"
    poll="${CAS_RELEASE_TRAIN_RECEIPTS_MERGEABLE_POLL_SECS:-1}"
    [[ "$tries" =~ ^[0-9]+$ ]] || tries=60
    [[ "$poll" =~ ^[0-9]+$ ]] || poll=1
    for ((i = 1; i <= tries; i++)); do
        view="$("$gh" pr view "$pr_number" -R "$repo" --json mergeable,state 2>&1 || true)"
        mergeable="$(printf '%s' "$view" | jq -r '.mergeable // empty' 2>/dev/null || true)"
        state="$(printf '%s' "$view" | jq -r '.state // empty' 2>/dev/null || true)"
        case "$mergeable" in
            MERGEABLE)
                return 0
                ;;
            CONFLICTING)
                printf 'ERROR receipts queue: PR #%s is conflicting and cannot be queued\n' "$pr_number" >&2
                return 1
                ;;
        esac
        if [[ "$state" == CLOSED || "$state" == MERGED ]]; then
            printf 'ERROR receipts queue: PR #%s is %s and cannot be queued\n' "$pr_number" "$state" >&2
            return 1
        fi
        if ((i < tries)); then
            sleep "$poll"
        fi
    done
    printf 'ERROR receipts queue: PR #%s mergeability remained %s after %s checks\n' \
        "$pr_number" "${mergeable:-UNKNOWN}" "$tries" >&2
    return 1
}

release_train_receipts_enqueue() {
    local gh="$1" repo="$2" pr_number="$3" pr_id="$4" tries poll i queue_output
    tries="${CAS_RELEASE_TRAIN_RECEIPTS_ENQUEUE_TRIES:-3}"
    poll="${CAS_RELEASE_TRAIN_RECEIPTS_ENQUEUE_POLL_SECS:-1}"
    [[ "$tries" =~ ^[0-9]+$ ]] || tries=3
    [[ "$poll" =~ ^[0-9]+$ ]] || poll=1
    for ((i = 1; i <= tries; i++)); do
        if queue_output="$("$gh" api graphql \
            -f query='mutation($id:ID!){enqueuePullRequest(input:{pullRequestId:$id}){mergeQueueEntry{position state}}}' \
            -F id="$pr_id" 2>&1)"; then
            if printf '%s' "$queue_output" | grep -q '"state"'; then
                printf '%s\n' "$queue_output"
                return 0
            fi
        fi
        if printf '%s' "$queue_output" | grep -Eiq 'mergeability check has not yet completed|UNPROCESSABLE|unprocessable'; then
            if ((i < tries)); then
                sleep "$poll"
                continue
            fi
        fi
        printf 'ERROR receipts queue: PR #%s was not queued: %s\n' "$pr_number" "$queue_output" >&2
        return 1
    done
    return 1
}

release_train_receipts() {
    local draft receipt landed branch receipt_worktree report_dir artifact body_file
    local repo_slug pr_url pr_number pr_id queue_output commit_sha existing_pr
    if [[ -s "$run_dir/receipts.pr" ]]; then
        printf 'receipts complete · receipt=%s\n' "$run_dir/receipts.pr"
        return 0
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
        if ! release_train_receipts_field "$key" | grep -q '[^[:space:]]'; then
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
    branch="docs/release-receipts-v$version"
    receipt_worktree="$run_dir/receipts-worktree"
    if [[ ! -e "$receipt_worktree/.git" ]]; then
        git -C "$worktree" worktree add -B "$branch" "$receipt_worktree" "$landed" >/dev/null
    fi
    mkdir -p "$receipt_worktree/docs/release-notes" "$receipt_worktree/docs/release-reports"
    cp "$draft" "$receipt_worktree/docs/release-notes/$(basename "$draft")"
    if ! grep -q '^## POSTED$' "$receipt_worktree/docs/release-notes/$(basename "$draft")"; then
        {
            cat "$receipt_worktree/docs/release-notes/$(basename "$draft")"
            printf '\n'
            release_train_receipts_posted_block
        } >"$receipt_worktree/.draft-with-posted"
        mv "$receipt_worktree/.draft-with-posted" \
            "$receipt_worktree/docs/release-notes/$(basename "$draft")"
    fi
    report_dir="$worktree/docs/release-reports"
    for artifact in "$report_dir/v$version"*; do
        [[ -f "$artifact" ]] || continue
        cp "$artifact" "$receipt_worktree/docs/release-reports/$(basename "$artifact")"
    done
    if ! find "$receipt_worktree/docs/release-reports" -maxdepth 1 -type f \
        -name "v$version*" -print -quit | grep -q .; then
        printf 'ERROR receipts report: no docs/release-reports/v%s* files found\n' "$version" >&2
        return 1
    fi
    git -C "$receipt_worktree" add docs/release-notes docs/release-reports
    if git -C "$receipt_worktree" diff --cached --quiet; then
        commit_sha="$(git -C "$receipt_worktree" rev-parse HEAD)"
    else
        git -C "$receipt_worktree" -c core.hooksPath=/dev/null commit \
            -m "docs(release): record v$version receipts and report" >/dev/null
        commit_sha="$(git -C "$receipt_worktree" rev-parse HEAD)"
    fi
    git -C "$receipt_worktree" push -u origin "$branch" >/dev/null
    repo_slug="${CAS_RELEASE_TRAIN_REPO:-Richards-LLC/cassy}"
    body_file="$run_dir/receipts-pr-body.md"
    release_train_receipts_standard_body "$landed" >"$body_file"
    existing_pr="$("${CAS_RELEASE_TRAIN_GH:-gh}" pr list -R "$repo_slug" --state open \
        --head "$branch" --json number --jq '.[0].number // empty' 2>/dev/null || true)"
    if [[ "$existing_pr" =~ ^[0-9]+$ ]]; then
        pr_number="$existing_pr"
        printf 'receipts reusing existing PR #%s for %s\n' "$pr_number" "$branch"
    else
        pr_url="$( "${CAS_RELEASE_TRAIN_GH:-gh}" pr create -R "$repo_slug" --base main \
            --head "$branch" --title "Docs: v$version release receipts" --body-file "$body_file")"
        pr_number="$(printf '%s\n' "$pr_url" | grep -oE '/pull/[0-9]+' | tail -n1 | cut -d/ -f3)"
    fi
    [[ "$pr_number" =~ ^[0-9]+$ ]] || {
        printf 'ERROR receipts PR: gh pr create returned no PR number\n' >&2
        return 1
    }
    pr_id="${CAS_RELEASE_TRAIN_RECEIPTS_PR_ID:-}"
    if [[ -z "$pr_id" ]]; then
        pr_id="$( "${CAS_RELEASE_TRAIN_GH:-gh}" pr view "$pr_number" -R "$repo_slug" --json id \
            | jq -r '.id // empty')"
    fi
    [[ -n "$pr_id" ]] || {
        printf 'ERROR receipts queue: PR #%s has no GraphQL id\n' "$pr_number" >&2
        return 1
    }
    release_train_receipts_wait_mergeable "${CAS_RELEASE_TRAIN_GH:-gh}" "$repo_slug" "$pr_number" || return 1
    queue_output="$(release_train_receipts_enqueue "${CAS_RELEASE_TRAIN_GH:-gh}" "$repo_slug" "$pr_number" "$pr_id")" || return 1
cat >"$run_dir/receipts.pr" <<EOF
PR_NUMBER=$pr_number
RECEIPTS_PR_NUMBER=$pr_number
PR_BRANCH=$branch
BASE_SHA=$landed
LANDED_SHA=$landed
RECEIPTS_LANDED_SHA=$landed
COMMIT_SHA=$commit_sha
QUEUE_RECEIPT=$queue_output
EOF
    printf 'receipts PR #%s queued · base=%s · commit=%s\n' "$pr_number" "$landed" "$commit_sha"
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
