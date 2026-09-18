#!/usr/bin/env bash
# Shared --cut ledger helpers. Stage implementations are sourced separately.

cut_stage_file() {
    printf '%s/stage.%s.done\n' "$run_dir" "$1"
}

cut_current_sha() {
    git -C "$worktree" rev-parse HEAD 2>/dev/null
}

cut_stage_done() {
    local stage="$1" receipt="$2" current
    [[ -s "$receipt" ]] || return 1
    current="$(cut_current_sha 2>/dev/null || true)"
    [[ "$current" =~ ^[0-9a-f]{40}$ ]] || return 1
    if [[ "$(tr -d '[:space:]' <"$receipt")" == "$current" ]]; then
        return 0
    fi
    git -C "$worktree" merge-base --is-ancestor \
        "$(tr -d '[:space:]' <"$receipt")" "$current" 2>/dev/null
}

cut_mark_stage_done() {
    local stage="$1" receipt
    receipt="$(cut_stage_file "$stage")"
    printf '%s\n' "$(cut_current_sha)" >"$receipt"
}

cut_external_var() {
    local stage="$1" normalized
    normalized="${stage//-/_}"
    printf 'CAS_RELEASE_TRAIN_%s_CMD\n' "${normalized^^}"
}

cut_has_external_stage() {
    local stage="$1" variable
    variable="$(cut_external_var "$stage")"
    [[ -n "${!variable:-}" ]]
}

cut_run_external_stage() {
    local stage="$1" variable command
    variable="$(cut_external_var "$stage")"
    command="${!variable:-}"
    [[ -n "$command" ]] || {
        printf 'stage %s has no implementation or command\n' "$stage" >&2
        return 127
    }
    (
        cd "$worktree"
        export CAS_RELEASE_TRAIN_RUN_DIR="$run_dir"
        export CAS_RELEASE_TRAIN_VERSION="$version"
        export CAS_RELEASE_TRAIN_WORKTREE="$worktree"
        export CAS_RELEASE_TRAIN_INVOCATION_KIND=internal
        export CAS_RELEASE_TRAIN_STAGE="$stage"
        export CAS_RELEASE_TRAIN_BLOCKER_STAGES="${CAS_RELEASE_TRAIN_BLOCKER_STAGES:-none}"
        export CUT_STAGE="$stage"
        bash -c "$command"
    )
}

cut_record_blocker() {
    local stage="$1" blocker_file="$run_dir/blockers.log"
    mkdir -p "$run_dir"
    if ! grep -Fqx "$stage" "$blocker_file" 2>/dev/null; then
        printf '%s\n' "$stage" >>"$blocker_file"
    fi
}

cut_stage_failure() {
    local stage="$1" detail="$2"
    cut_record_blocker "$stage"
    printf 'BLOCKER %s: %s\n' "$stage" "$detail" >&2
    printf 'receipt: %s\n' "$(cut_stage_file "$stage")" >&2
    printf 'resume: %q %q %q --cut --resume\n' "$0" "$version" "$worktree" >&2
}

cut_run_stage() {
    local stage="$1" function_name="cut_stage_${1//-/_}" receipt
    receipt="$(cut_stage_file "$stage")"
    if cut_stage_done "$stage" "$receipt"; then
        printf 'stage %s: skipped (receipt %s matches current history)\n' "$stage" "$receipt"
        return 0
    fi
    printf 'stage %s: start\n' "$stage"
    export CAS_RELEASE_TRAIN_STAGE="$stage"
    if declare -F "$function_name" >/dev/null 2>&1; then
        "$function_name" || return 1
    elif cut_has_external_stage "$stage"; then
        cut_run_external_stage "$stage" || return 1
    else
        cut_stage_failure "$stage" "no sourced stage body or command"
        return 1
    fi
    cut_mark_stage_done "$stage"
    printf 'stage %s: done sha=%s receipt=%s\n' \
        "$stage" "$(tr -d '[:space:]' <"$receipt")" "$receipt"
}

cut_run() {
    local resume="${1:-false}" stage
    mkdir -p "$run_dir"
    write_run_env "$run_dir/run.env"
    export CAS_RELEASE_TRAIN_INVOCATION_KIND=internal
    export CAS_RELEASE_TRAIN_RUN_DIR="$run_dir"
    if [[ "$resume" == true && -s "$run_dir/blockers.log" ]]; then
        export CAS_RELEASE_TRAIN_BLOCKER_STAGES="$(paste -sd, "$run_dir/blockers.log")"
    fi
    printf 'cut start version=%s worktree=%s resume=%s\n' "$version" "$worktree" "$resume"
    for stage in preflight assemble prep ledger gate pr-body pipeline publish \
        post-publication announce report receipts host-update; do
        if ! cut_run_stage "$stage"; then
            cut_stage_failure "$stage" "stage failed; inspect the receipt and log"
            return 1
        fi
        if [[ "${CAS_RELEASE_TRAIN_CUT_STOP_AFTER:-}" == "$stage" ]]; then
            printf 'stopped after stage %s; resume with --resume\n' "$stage" >&2
            return 1
        fi
    done
    printf 'cut complete version=%s worktree=%s\n' "$version" "$worktree"
}
