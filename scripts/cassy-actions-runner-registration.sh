#!/usr/bin/env bash

set_runner_group_args() {
    local registration="$1"
    local runner_group="$2"
    local -n group_args_out="$3"
    group_args_out=(--runnergroup "$runner_group")
}
