#!/usr/bin/env bash

cut_stage_host_update() {
    if cut_has_external_stage host-update; then
        cut_run_external_stage host-update
    else
        printf 'host update proof deferred to the release host stage\n'
    fi
}
