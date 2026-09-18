#!/usr/bin/env bash

cut_stage_gate() {
    local gate_done="$run_dir/gate.done" poll="${CAS_RELEASE_TRAIN_CUT_POLL_SECS:-1}"
    local tries="${CAS_RELEASE_TRAIN_CUT_GATE_TRIES:-3600}" pid i rc
    CAS_RELEASE_TRAIN_INVOCATION_KIND=internal \
    CAS_RELEASE_TRAIN_STAGE=gate CAS_RELEASE_TRAIN_RUN_DIR="$run_dir" \
    CAS_RELEASE_TRAIN_BLOCKER_STAGES="${CAS_RELEASE_TRAIN_BLOCKER_STAGES:-none}" \
        "$0" "$version" "$worktree" --gate
    for ((i = 1; i <= tries; i++)); do
        if [[ -s "$gate_done" ]]; then
            rc="$(tr -d '[:space:]' <"$gate_done")"
            [[ "$rc" == 0 ]] || {
                printf 'gate failed with status %s; inspect %s/gate.log\n' "$rc" "$run_dir" >&2
                return 1
            }
            return 0
        fi
        pid="$(cat "$run_dir/gate.pid" 2>/dev/null || true)"
        if [[ -n "$pid" ]] && ! kill -0 "$pid" 2>/dev/null; then
            printf 'gate exited without a receipt; inspect %s/gate.log\n' "$run_dir" >&2
            return 1
        fi
        sleep "$poll"
    done
    printf 'gate did not finish within %ss; inspect %s/gate.log\n' \
        "$((tries * poll))" "$run_dir" >&2
    return 1
}
