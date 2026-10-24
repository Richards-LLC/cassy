#!/usr/bin/env bash

set_runner_group_args() {
    local registration="$1"
    local runner_group="$2"
    local -n group_args_out="$3"
    case "$registration" in
        org) group_args_out=(--runnergroup "$runner_group") ;;
        # shellcheck disable=SC2034 # The caller expands this nameref as the config argument array.
        repo) group_args_out=() ;;
        *)
            printf 'RUNNER_REGISTRATION must be org or repo; got %s\n' "$registration" >&2
            return 1
            ;;
    esac
}
