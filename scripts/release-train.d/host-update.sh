#!/usr/bin/env bash

# Update this host to the release and prove cas, the hub and the refresh
# receipt all report it (host-update.json in the run directory). A deferred or
# no-op update is a named blocker, never a done receipt (3.27.6 gap).
release_train_host_update() {
    mkdir -p "$run_dir"
    python3 "$script_dir/release-host-update.py" "$version" "$run_dir/host-update.json"
}

cut_stage_host_update() {
    if cut_has_external_stage host-update; then
        cut_run_external_stage host-update
    else
        release_train_host_update
    fi
}
