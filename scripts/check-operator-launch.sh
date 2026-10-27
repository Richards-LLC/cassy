#!/usr/bin/env bash
# Operator launch smoke (cas-e719): can an operator launch the candidate `cas`?
#
# 3.51.0 refused `cas factory` and `cas claude <account>` for every operator
# on the default factory.nice_cargo=true: `cas` exported its own build knob,
# and its operator-only gate then read that variable as an agent marker. Unit
# tests built synthetic environments, so nothing launched a real binary from a
# clean operator shell.
#
# This installs nothing: it runs the given binary under a throwaway HOME and a
# `cas init` project (default config), in a clean operator-like environment:
#   - only HOME/PATH/TERM/locale plus the Claude Code preferences operators
#     export from their shell rc (CLAUDE_CODE_NATIVE_CURSOR, _NO_FLICKER);
#   - a login shell inside a pseudo-terminal (`script`);
#   - on Linux, a transient systemd user unit, so the process leaves the
#     caller's agent ancestry and cgroup (the release gate is driven by one).
# Each operator-only entry point runs with --check-launch, which stops at the
# launch boundary. The operator must pass every one; the same invocation with
# CAS_AGENT_ROLE set must be refused.
#
# Usage: scripts/check-operator-launch.sh <cas-binary>
set -euo pipefail

cas_bin="${1:?usage: check-operator-launch.sh <cas-binary>}"
[[ -x "$cas_bin" ]] || { printf 'operator launch smoke: %s is not executable\n' "$cas_bin" >&2; exit 2; }
cas_bin="$(cd "$(dirname "$cas_bin")" && pwd -P)/$(basename "$cas_bin")"

scratch="$(mktemp -d "${TMPDIR:-/tmp}/cas-operator-launch.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
home="$scratch/home"
project="$scratch/project"
mkdir -p "$home" "$project" "$scratch/bin"
ln -s "$cas_bin" "$scratch/bin/cas"
# `cas claude <account>` refuses unknown account names (3.84), so the smoke
# launches a fixture account folder in the throwaway HOME rather than a made-up
# name. It holds no credentials; --check-launch stops before any login.
mkdir -p "$home/.claude-smoke"

# The operator's own shell: nothing from the caller's environment survives.
operator_env=(
    "HOME=$home" "USER=${USER:-operator}" "LOGNAME=${LOGNAME:-operator}" "SHELL=/bin/bash"
    "PATH=$scratch/bin:/usr/local/bin:/usr/bin:/bin" "TERM=xterm-256color" "LANG=C.UTF-8"
    "CLAUDE_CODE_NATIVE_CURSOR=1" "CLAUDE_CODE_NO_FLICKER=1"
)

# Leave the caller's process tree and cgroup where the host allows it. macOS
# has neither an ancestry nor a cgroup signal, so a clean env is the operator.
detach=()
if [[ "$(uname -s)" == Linux ]] && command -v systemd-run >/dev/null 2>&1 \
    && systemd-run --user --quiet --wait --pipe --collect true >/dev/null 2>&1; then
    detach=(systemd-run --user --quiet --wait --pipe --collect "--working-directory=$project" --)
fi

# Run one command line in a login shell on a pseudo-terminal, from the project.
in_operator_terminal() {
    local command_line="cd '$project' && $1" extra_env="${2:-}"
    local -a env_args=("${operator_env[@]}")
    [[ -z "$extra_env" ]] || env_args+=("$extra_env")
    if [[ "$(uname -s)" == Darwin ]]; then
        ${detach[@]+"${detach[@]}"} env -i "${env_args[@]}" script -q /dev/null /bin/bash -lc "$command_line"
    else
        ${detach[@]+"${detach[@]}"} env -i "${env_args[@]}" script -qec "/bin/bash -lc \"$command_line\"" /dev/null
    fi
}

# A real `cas init` project: its default config is what the operator launches
# with, including factory.nice_cargo=true.
if ! output="$(in_operator_terminal 'git init -q . 2>/dev/null; cas init --yes --no-integrations' 2>&1)"; then
    printf 'FAIL operator launch smoke: cas init failed in a clean operator project\n%s\n' "$output"
    exit 1
fi

entry_points=(
    'cas factory --check-launch'
    'cas claude smoke --check-launch'
    'cas hub pair --origin https://commander.invalid --check-launch'
    'cas hub authorize SMOK-E000 --check-launch'
)
failures=0
for entry in "${entry_points[@]}"; do
    status=0
    output="$(in_operator_terminal "$entry" 2>&1)" || status=$?
    if [[ "$status" -eq 0 && "$output" == *'launch check: '* && "$output" != *refused* ]]; then
        printf 'ok   operator: %s\n' "$entry"
    else
        printf 'FAIL operator: %s (exit %s)\n%s\n' "$entry" "$status" "$output"
        failures=$((failures + 1))
    fi
    status=0
    output="$(in_operator_terminal "$entry" CAS_AGENT_ROLE=worker 2>&1)" || status=$?
    if [[ "$status" -ne 0 && "$output" == *'refused: CAS_AGENT_ROLE is set'* ]]; then
        printf 'ok   agent refused: %s\n' "$entry"
    else
        printf 'FAIL agent not refused: %s (exit %s)\n%s\n' "$entry" "$status" "$output"
        failures=$((failures + 1))
    fi
done

if [[ "$failures" -ne 0 ]]; then
    printf 'FAIL operator launch smoke: %s check(s) failed; an operator could not launch %s\n' \
        "$failures" "$cas_bin"
    exit 1
fi
printf 'PASS operator launch smoke: %s entry points launch for the operator and refuse an agent (%s)\n' \
    "${#entry_points[@]}" "${detach[0]:-direct}"
