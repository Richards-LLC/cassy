#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/cassy-actions-runner-registration.sh
source "$script_dir/cassy-actions-runner-registration.sh"

assert_group_args() {
    local registration="$1"
    shift
    local -a expected=("$@") actual=()
    set_runner_group_args "$registration" cassy-public-trusted actual
    if (( ${#actual[@]} != ${#expected[@]} )); then
        printf 'FAIL %s registration expected %d runner-group args, got %d\n' \
            "$registration" "${#expected[@]}" "${#actual[@]}" >&2
        exit 1
    fi
    for index in "${!expected[@]}"; do
        if [[ "${actual[index]}" != "${expected[index]}" ]]; then
            printf 'FAIL %s registration runner-group arg %d: expected %q, got %q\n' \
                "$registration" "$index" "${expected[index]}" "${actual[index]}" >&2
            exit 1
        fi
    done
}

assert_group_args org --runnergroup cassy-public-trusted
assert_group_args repo
printf 'PASS repository runner registration omits the organization-only runner group\n'
