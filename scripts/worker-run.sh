# shellcheck shell=bash
# cas-22c6 (GH #2286): a proof script that reads a factory worker's checkout
# holds it for its whole life, so a queued task start waits (G-165) instead of
# switching the tree under the run. Source this file, call
# `worker_run_begin <dir inside the checkout>` before reading the tree, and call
# `worker_run_end "$status"` from the script's EXIT trap. HUP, INT and TERM exit
# through that trap; the bound pid covers a SIGKILL. Without CAS_ROOT (CI, an
# operator checkout) both are no-ops.
worker_run_id=""

worker_run_begin() {
    [[ -n "${CAS_ROOT:-}" ]] || return 0
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
    if ! worker_run_id="$("${CAS_BIN:-cas}" factory worker-run-begin --cas-root "$CAS_ROOT" --repo "$1" --pid "$$")"; then
        printf '%s: this run is not registered with Cassy, so a task start may switch the checkout under it\n' \
            "${0##*/}" >&2
        worker_run_id=""
    fi
}

worker_run_end() {
    [[ -n "$worker_run_id" ]] || return 0
    "${CAS_BIN:-cas}" factory worker-run-end --cas-root "$CAS_ROOT" --id "$worker_run_id" --code "$1" >/dev/null || :
}
