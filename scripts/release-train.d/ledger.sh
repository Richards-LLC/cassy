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

# A squash landing need not contain the preflight/prep commits. Once the
# pipeline or publisher has succeeded, its receipt is the resume boundary.
cut_resume_boundary() {
    local landed pipeline publish
    [[ -s "$run_dir/landed-main.sha" ]] || return 1
    landed="$(tr -d '[:space:]' <"$run_dir/landed-main.sha" 2>/dev/null || true)"
    [[ "$landed" =~ ^[0-9a-f]{40}$ ]] || return 1
    publish="$(cut_stage_file publish)"
    pipeline="$(cut_stage_file pipeline)"
    if [[ -s "$publish" && "$(tr -d '[:space:]' <"$publish")" == "$landed" ]] \
        && cut_stage_done publish "$publish"; then
        printf 'post-publication\n'
    elif [[ -s "$pipeline" && -s "$run_dir/pipeline.done" \
        && "$(tr -d '[:space:]' <"$run_dir/pipeline.done")" == MERGED ]] \
        && { cut_stage_done pipeline "$pipeline" \
            || git -C "$worktree" merge-base --is-ancestor "$landed" HEAD 2>/dev/null; }; then
        # Also covers a publisher that moved HEAD, then failed before its
        # stage receipt. Its retry still owns publication checks.
        [[ "$(tr -d '[:space:]' <"$pipeline")" =~ ^[0-9a-f]{40}$ ]] || return 1
        git -C "$worktree" cat-file -e "$(tr -d '[:space:]' <"$pipeline")^{commit}" 2>/dev/null || return 1
        printf 'publish\n'
    else
        return 1
    fi
}

cut_resume_outputs() {
    python3 "$script_dir/release-train-resume.py" "$1" "$worktree" "$run_dir" \
        "$version" "$(release_train_announce_draft_path)"
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

# Timed stage events for end-to-end release metrics (cas-a629): one
# `<epoch>\t<stage>\t<start|done|blocked>` row per event, never rewritten, so
# scripts/release-metrics.py can price every blocker from its block to its
# stage's next completion.
cut_stage_event() {
    mkdir -p "$run_dir"
    printf '%s\t%s\t%s\n' "$(date -u +%s)" "$1" "$2" >>"$run_dir/stage-events.tsv"
}

cut_record_blocker() {
    local stage="$1" blocker_file="$run_dir/blockers.log"
    mkdir -p "$run_dir"
    if ! grep -Fqx "$stage" "$blocker_file" 2>/dev/null; then
        printf '%s\n' "$stage" >>"$blocker_file"
    fi
    cut_stage_event "$stage" blocked
}

cut_stage_failure() {
    local stage="$1" detail="$2"
    cut_record_blocker "$stage"
    printf 'BLOCKER %s: %s\n' "$stage" "$detail" >&2
    printf 'receipt: %s\n' "$(cut_stage_file "$stage")" >&2
    printf 'resume: %q %q %q --cut --resume\n' "$0" "$version" "$worktree" >&2
}

cut_run_stage() {
    local stage="$1" function_name="cut_stage_${1//-/_}" receipt status=0
    receipt="$(cut_stage_file "$stage")"
    if [[ "$stage" == receipts ]]; then
        python3 "$script_dir/release-learning.py" --check "$worktree" "$run_dir" || return 1
    fi
    if cut_stage_done "$stage" "$receipt"; then
        # A historical done marker cannot prove a newly advanced main or a
        # replaced installed binary. Re-check rule-175 on every completion.
        if [[ "$stage" == host-update ]]; then
            release_train_delivery_completion || return
        fi
        printf 'stage %s: skipped (receipt %s matches current history)\n' "$stage" "$receipt"
        return 0
    fi
    printf 'stage %s: start\n' "$stage"
    cut_stage_event "$stage" start
    export CAS_RELEASE_TRAIN_STAGE="$stage"
    if declare -F "$function_name" >/dev/null 2>&1; then
        "$function_name" || status=$?
    elif cut_has_external_stage "$stage"; then
        cut_run_external_stage "$stage" || status=$?
    else
        cut_stage_failure "$stage" "no sourced stage body or command"
        return 1
    fi
    case "$stage" in
        post-publication|announce|report|receipts)
            # Failed stages can leave partial, legitimate release evidence.
            # Bind its exact bytes to this run before printing --resume.
            cut_resume_outputs record || return 1
            ;;
    esac
    [[ "$status" == 0 ]] || return "$status"
    if [[ "$stage" == assemble ]]; then
        python3 "$script_dir/release-integrate.py" "$worktree" --record-input || return 1
    fi
    cut_mark_stage_done "$stage"
    cut_stage_event "$stage" done
    printf 'stage %s: done sha=%s receipt=%s\n' \
        "$stage" "$(tr -d '[:space:]' <"$receipt")" "$receipt"
}

cut_stage_ledger() {
    if cut_has_external_stage ledger; then
        cut_run_external_stage ledger
        return
    fi
    local generator="$worktree/scripts/gen-builtin-reference-history.sh"
    [[ -x "$generator" ]] || return 0
    (
        cd "$worktree"
        "$generator"
    )
    if ! git -C "$worktree" diff --quiet -- cas-cli/src/builtins/reference-history.json; then
        git -C "$worktree" add -- cas-cli/src/builtins/reference-history.json
        git -C "$worktree" -c core.hooksPath=/dev/null commit \
            -m "build: regenerate builtin reference ledger" >/dev/null
    fi
}

cut_run() {
    local resume="${1:-false}" stage boundary="" embargo
    mkdir -p "$run_dir"
    write_run_env "$run_dir/run.env"
    export CAS_RELEASE_TRAIN_INVOCATION_KIND=internal
    export CAS_RELEASE_TRAIN_RUN_DIR="$run_dir"
    # Keep an explicit embargo across resumes. An explicitly empty value
    # lifts it; omission does not silently authorize announcement writes.
    if [[ -z "${CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO+x}" && -s "$run_dir/announcement-embargo.txt" ]]; then
        export CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO="$(cat "$run_dir/announcement-embargo.txt")"
    fi
    embargo="${CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO:-}"
    if [[ -n "${embargo//[[:space:]]/}" ]]; then
        printf '%s\n' "$CAS_RELEASE_TRAIN_ANNOUNCEMENT_EMBARGO" >"$run_dir/announcement-embargo.txt"
    else
        rm -f "$run_dir/announcement-embargo.txt"
    fi
    if [[ "$resume" == true && -s "$run_dir/blockers.log" ]]; then
        export CAS_RELEASE_TRAIN_BLOCKER_STAGES="$(paste -sd, "$run_dir/blockers.log")"
    fi
    # The first cut's start and the release request are recorded once; a
    # resume never moves either clock (cas-a629). CAS_RELEASE_TRAIN_REQUESTED_AT
    # (epoch seconds or ISO 8601 UTC) names when the release was asked for.
    [[ -s "$run_dir/cut.start.epoch" ]] || date -u +%s >"$run_dir/cut.start.epoch"
    if [[ ! -s "$run_dir/release.request.epoch" && -n "${CAS_RELEASE_TRAIN_REQUESTED_AT:-}" ]]; then
        python3 "$script_dir/release-metrics.py" --normalize-request \
            "$CAS_RELEASE_TRAIN_REQUESTED_AT" >"$run_dir/release.request.epoch" || {
            rm -f "$run_dir/release.request.epoch"
            printf 'error: CAS_RELEASE_TRAIN_REQUESTED_AT must be epoch seconds or ISO 8601 UTC, not after now\n' >&2
            return 2
        }
    fi
    printf 'cut start version=%s worktree=%s resume=%s\n' "$version" "$worktree" "$resume"
    if [[ "$resume" == true ]]; then
        boundary="$(cut_resume_boundary || true)"
        if [[ -n "$boundary" ]]; then
            if [[ "$boundary" == publish ]]; then
                [[ -z "$(git -C "$worktree" status --porcelain)" ]] || {
                    cut_preflight_block clean-worktree "release worktree has uncommitted changes before publish"
                    return 1
                }
            elif ! cut_resume_outputs check; then
                cut_preflight_block clean-worktree "changes do not match this run's post-publication outputs"
                return 1
            fi
        elif ! python3 "$script_dir/release-integrate.py" "$worktree" --resume-check; then
            cut_stage_failure integration-refresh "cannot refresh assembly input; inspect the named blocker"
            return 1
        fi
    fi
    for stage in preflight assemble prep ledger gate pr-body pipeline publish \
        post-publication announce report receipts host-update; do
        if [[ -n "$boundary" && "$stage" != "$boundary" ]]; then
            printf 'stage %s: skipped (completed publication boundary; resume at %s)\n' "$stage" "$boundary"
            continue
        fi
        boundary=""
        if [[ -s "$run_dir/announcement-embargo.txt" && "$stage" =~ ^(announce|report|receipts)$ ]]; then
            printf 'stage %s: pending (explicit announcement embargo; runtime publication continues)\n' "$stage"
            continue
        fi
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
