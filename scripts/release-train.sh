#!/usr/bin/env bash
# Owns one release-gate RUN: its directory, its receipts, and its process.
#
# Why this exists (cas-5212). The release train used to be driven by wrappers
# hand-written per release under ~/.cas/artifacts/release/v<version>/, and both
# the artifacts directory and the ad-hoc "find the gate" commands were keyed on
# the VERSION STRING. A version string does not identify a run: on 2026-09-04
# two supervisors gated 3.15.2 concurrently from different epic worktrees
# (.cas/epic-4f6f-merge and .cas/epic-cas-8094-merge). Both resolved the same
# directory, so gate.log and gate.done were mutually overwritable and an agent
# reported one supervisor's gate state to the other. A `pgrep -f
# 'release-gate.sh 3.15.2' | head -1` typed during that incident matched BOTH
# processes; acting on `head -1` picks by pid ordering, not by ownership.
#
# The rules this script exists to make unbreakable:
#   * the run directory is keyed by version AND worktree, never version alone;
#   * a run is located by a PID this script recorded, never by a name pattern;
#   * `--stop` signals only that recorded pid, so a sibling run survives.
#
# Usage:
#   scripts/release-train.sh <version> <epic-worktree> --gate
#   scripts/release-train.sh <version> <epic-worktree> --status
#   scripts/release-train.sh <version> <epic-worktree> --stop
#   scripts/release-train.sh <version> <epic-worktree> --print-run-dir
#
# Environment seams (defaults are the real thing; the self-test overrides them):
#   CAS_RELEASE_ARTIFACTS_ROOT   default ~/.cas/artifacts/release
#   CAS_RELEASE_TRAIN_GATE_CMD   default <worktree>/scripts/release-gate.sh
#   CAS_RELEASE_TRAIN_PROXY_TOML default <main checkout>/.cas/proxy.toml
set -euo pipefail

usage() {
    printf 'Usage: %s <version> <epic-worktree> [--gate|--status|--stop|--print-run-dir]\n' "$0"
}

version="${1:-}"
worktree="${2:-}"
action="${3:---gate}"

if [[ -z "$version" || -z "$worktree" ]]; then
    usage >&2
    exit 2
fi
if [[ ! -d "$worktree" ]]; then
    printf 'error: epic worktree %s does not exist\n' "$worktree" >&2
    exit 2
fi

worktree="$(cd "$worktree" && pwd)"
worktree_name="$(basename "$worktree")"
artifacts_root="${CAS_RELEASE_ARTIFACTS_ROOT:-$HOME/.cas/artifacts/release}"
# The identity of a run: which version, from which worktree. Two supervisors
# cutting the same version from different epics get different directories.
run_dir="$artifacts_root/v$version-$worktree_name"
pid_file="$run_dir/gate.pid"

# The pid recorded for this run, if it is still alive. Liveness is asked of the
# recorded pid directly — never inferred from a process name.
live_gate_pid() {
    local pid
    pid="$(cat "$pid_file" 2>/dev/null || true)"
    [[ -n "$pid" ]] || return 1
    kill -0 "$pid" 2>/dev/null || return 1
    printf '%s\n' "$pid"
}

write_run_env() {
    local tip
    tip="$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    cat >"$run_dir/run.env" <<EOF
version=$version
worktree=$worktree
worktree_name=$worktree_name
repository=$(git -C "$worktree" rev-parse --show-toplevel 2>/dev/null || echo unknown)
tip=$tip
started_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
started_by_pid=$$
EOF
}

case "$action" in
    --print-run-dir)
        printf '%s\n' "$run_dir"
        exit 0
        ;;
    --status)
        printf 'run directory: %s\n' "$run_dir"
        if [[ -f "$run_dir/run.env" ]]; then
            cat "$run_dir/run.env"
        else
            printf 'no run recorded yet\n'
        fi
        if pid="$(live_gate_pid)"; then
            printf 'gate: running (pid %s)\n' "$pid"
        elif [[ -f "$run_dir/gate.done" ]]; then
            printf 'gate: finished with status %s\n' "$(cat "$run_dir/gate.done")"
        else
            printf 'gate: not running\n'
        fi
        exit 0
        ;;
    --stop)
        if pid="$(live_gate_pid)"; then
            # Only ever the pid this run recorded. No pattern, no `head -1`.
            kill -TERM "$pid"
            printf 'signalled gate pid %s for %s\n' "$pid" "$run_dir"
            exit 0
        fi
        printf 'no live gate recorded for %s; nothing signalled\n' "$run_dir" >&2
        exit 1
        ;;
    --gate) ;;
    *)
        usage >&2
        exit 2
        ;;
esac

mkdir -p "$run_dir"

# Refuse rather than race. The check is against the pid this run recorded, so a
# sibling supervisor's gate is invisible here — as it should be.
if pid="$(live_gate_pid)"; then
    owner_worktree="$(sed -n 's/^worktree=//p' "$run_dir/run.env" 2>/dev/null || true)"
    printf 'refusing to start: a gate for %s is already in progress for worktree %s (pid %s).\n' \
        "$version" "${owner_worktree:-$worktree}" "$pid" >&2
    printf 'Inspect it with `%s %s %s --status`, wait for it to finish, or stop it with `%s %s %s --stop` — only if that run is yours.\n' \
        "$0" "$version" "$worktree" "$0" "$version" "$worktree" >&2
    exit 3
fi

gate_cmd="${CAS_RELEASE_TRAIN_GATE_CMD:-$worktree/scripts/release-gate.sh}"
if [[ ! -x "$gate_cmd" ]]; then
    printf 'error: gate command %s is not executable\n' "$gate_cmd" >&2
    exit 2
fi

# The host-local .cas/proxy.toml leaks into hermetic proxy tests through the
# ancestor lookup (cas-4ccc), so it is moved aside for the run and restored on
# exit — including when the gate is killed.
main_checkout="$(git -C "$worktree" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##' || true)"
proxy_toml="${CAS_RELEASE_TRAIN_PROXY_TOML:-${main_checkout:-$worktree}/.cas/proxy.toml}"
proxy_aside="$proxy_toml.gate-aside"

restore_proxy() {
    if [[ -f "$proxy_aside" ]]; then
        mv "$proxy_aside" "$proxy_toml"
        printf 'proxy.toml restored %s\n' "$(date -u +%H:%M:%SZ)"
    fi
}
trap restore_proxy EXIT

if [[ -f "$proxy_toml" ]]; then
    mv "$proxy_toml" "$proxy_aside"
    printf 'proxy.toml moved aside %s\n' "$(date -u +%H:%M:%SZ)"
fi

write_run_env
rm -f "$run_dir/gate.done"

printf 'gate start %s version=%s worktree=%s tip=%s\n' \
    "$(date -u +%H:%M:%SZ)" "$version" "$worktree" \
    "$(git -C "$worktree" rev-parse --short HEAD 2>/dev/null || echo unknown)"
printf 'run directory: %s\n' "$run_dir"

(
    cd "$worktree"
    [[ -x "$PWD/.context/zig/zig" ]] && export ZIG="$PWD/.context/zig/zig"
    export CAS_RELEASE_GATE_HOME_DIR="${CAS_RELEASE_GATE_HOME_DIR:-/var/tmp/cas-release-gate}"
    exec "$gate_cmd" "$version"
) >"$run_dir/gate.log" 2>&1 &
gate_pid=$!
printf '%s\n' "$gate_pid" >"$pid_file"

set +e
wait "$gate_pid"
rc=$?
set -e
printf '%s\n' "$rc" >"$run_dir/gate.done"
printf 'gate rc=%s end %s\n' "$rc" "$(date -u +%H:%M:%SZ)"
exit "$rc"
