#!/usr/bin/env bash
# Operator-run, reversible migration of the soundwave runner cache to Shockwave.
# Run as the logged-in GitHub operator; this script uses sudo only for host writes.
set -Eeuo pipefail

repo=Richards-LLC/cassy
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
services=(cassy-actions-runner.service cassy-actions-runner-2.service)
route_vars=(CASSY_SELF_HOSTED_PILOT CASSY_MERGE_QUEUE_SELF_HOSTED CASSY_RELEASE_SELF_HOSTED)
runner_user=cassy-actions
testing="${CASSY_ACTIONS_CUTOVER_TESTING:-0}"

if [[ "$testing" == 1 ]]; then
    test_root="${CASSY_ACTIONS_CUTOVER_TEST_ROOT:?test mode requires CASSY_ACTIONS_CUTOVER_TEST_ROOT}"
    [[ "$test_root" == /* && "$test_root" != / && -f "$test_root/.cassy-actions-cutover-test-root" ]] \
        || { printf 'error: invalid cutover test root\n' >&2; exit 2; }
    runner_root="$test_root/var/lib/cassy-actions"
    source_path="$runner_root/cache"
    volume_path="$test_root/mnt/shockwave"
    dest_path="$volume_path/home/pippenz/cassy-actions/cache"
    fstab_path="$test_root/etc/fstab"
    systemd_dir="$test_root/etc/systemd/system"
    artifact_root="${CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT:-$test_root/artifacts}"
    representative_find_size=+0c
    representative_required=3
else
    runner_root=/var/lib/cassy-actions
    source_path="$runner_root/cache"
    volume_path=/mnt/shockwave
    dest_path="$volume_path/home/pippenz/cassy-actions/cache"
    fstab_path=/etc/fstab
    systemd_dir=/etc/systemd/system
    artifact_root="${CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT:-/home/pippenz/.cas/artifacts/cas-a352}"
    representative_find_size=+100M
    representative_required=3
fi

fstab_line="$dest_path $source_path none bind,x-systemd.requires-mounts-for=$volume_path 0 0"
state_file=
rollback_path=
renamed=0
host_mutated=0
services_may_be_stopped=0
routes_captured=0
pruner_existed=0
mount_guard_existed=0

fail() {
    printf 'error: %s\n' "$1" >&2
    return 1
}

route_error_file() {
    local name="$1"
    printf '%s.route-%s.stderr\n' "${state_file:-$artifact_root/operator-cutover}" "$name"
}

route_get() {
    local name="$1" value error_file
    error_file="$(route_error_file "$name")"
    if value="$(gh api "repos/$repo/actions/variables/$name" --jq '.value' 2>"$error_file")"; then
        printf '%s\n' "$value"
        return 0
    fi
    if grep -Eq '\(HTTP 404\)[[:space:]]*$' "$error_file"; then
        printf '<unset>\n'
        return 0
    fi
    printf 'route read failed for %s: ' "$name" >&2
    cat "$error_file" >&2
    return 1
}

route_set() {
    local name="$1" value="$2" error_file
    error_file="$(route_error_file "$name").restore"
    if [[ "$value" != '<unset>' ]]; then
        gh variable set "$name" --repo "$repo" --body "$value"
        return
    fi
    if gh variable delete "$name" --repo "$repo" 2>"$error_file"; then
        return 0
    fi
    if grep -Eq '\(HTTP 404\)[[:space:]]*$' "$error_file"; then
        return 0
    fi
    printf 'route deletion failed for %s: ' "$name" >&2
    cat "$error_file" >&2
    return 1
}

restore_routes() {
    local status=0
    [[ "$routes_captured" == 1 ]] || return 0
    route_set CASSY_SELF_HOSTED_PILOT "$original_pilot" || status=1
    route_set CASSY_MERGE_QUEUE_SELF_HOSTED "$original_merge_queue" || status=1
    route_set CASSY_RELEASE_SELF_HOSTED "$original_release" || status=1
    return "$status"
}

disable_routes() {
    local name
    for name in "${route_vars[@]}"; do
        gh variable set "$name" --repo "$repo" --body disabled || return 1
    done
}

runner_snapshot() {
    gh api orgs/Richards-LLC/actions/runners --jq \
        '.runners[] | select(.name == "soundwave-cas-ci" or .name == "soundwave-cas-ci-2") | {name,status,busy}'
}

busy_runner_count() {
    gh api orgs/Richards-LLC/actions/runners --jq \
        '[.runners[] | select((.name == "soundwave-cas-ci" or .name == "soundwave-cas-ci-2") and .busy == true)] | length'
}

wait_for_idle_online() {
    local attempt count
    for attempt in {1..60}; do
        if ! count="$(gh api orgs/Richards-LLC/actions/runners --jq \
            '[.runners[] | select((.name == "soundwave-cas-ci" or .name == "soundwave-cas-ci-2") and .status == "online" and .busy == false)] | length')"; then
            return 1
        fi
        [[ "$count" == 2 ]] && return 0
        sleep 1
    done
    return 1
}

prove_no_job_process() {
    if pgrep -u "$runner_user" -f 'Runner.Worker' >/dev/null; then
        pgrep -a -u "$runner_user" -f 'Runner.Worker' >&2 || true
        fail 'a runner job process is active'
    fi
}

assert_no_active_jobs() {
    local busy
    busy="$(busy_runner_count)" || return 1
    [[ "$busy" =~ ^[0-9]+$ ]] || fail "invalid busy-runner count: $busy"
    [[ "$busy" == 0 ]] || fail "$busy runner job(s) are active"
    prove_no_job_process
}

drain_and_stop_for_cutover() {
    wait_for_idle_online || { fail 'both runners did not become online and idle while routing was disabled'; return 1; }
    assert_no_active_jobs || return 1
    services_may_be_stopped=1
    sudo systemctl stop "${services[@]}" || return 1
    prove_no_job_process || return 1
}

drain_and_stop_for_rollback() {
    local service active=0 attempt busy
    for service in "${services[@]}"; do
        if systemctl is-active --quiet "$service"; then
            active=$((active + 1))
        fi
    done
    [[ "$active" == 0 ]] && { prove_no_job_process || return 1; return 0; }

    for attempt in {1..60}; do
        if busy="$(busy_runner_count)" && [[ "$busy" == 0 ]] \
            && ! pgrep -u "$runner_user" -f 'Runner.Worker' >/dev/null; then
            sudo systemctl stop "${services[@]}" || return 1
            prove_no_job_process || return 1
            return 0
        fi
        sleep 1
    done
    fail 'rollback refused to stop services because active-job absence was not proven'
}

verify_destination_on_shockwave() {
    local volume_device dest_device volume_real dest_real dest_mount expected_dest
    mountpoint -q "$volume_path" || fail "$volume_path is not a mount point"
    volume_real="$(realpath -e "$volume_path")" || return 1
    dest_real="$(realpath -e "$dest_path")" || return 1
    expected_dest="$volume_real/home/pippenz/cassy-actions/cache"
    [[ "$dest_real" == "$expected_dest" ]] \
        || fail "destination resolves to $dest_real, expected exact Shockwave subtree $expected_dest"
    volume_device="$(findmnt -n -o MAJ:MIN -T "$volume_path")" || return 1
    dest_device="$(findmnt -n -o MAJ:MIN -T "$dest_path")" || return 1
    [[ "$dest_device" == "$volume_device" ]] \
        || fail "destination device $dest_device differs from Shockwave device $volume_device"
    dest_mount="$(findmnt -n -o TARGET -T "$dest_path")" || return 1
    [[ "$(realpath -e "$dest_mount")" == "$volume_real" ]] \
        || fail "destination is mounted through $dest_mount, not exact Shockwave mount $volume_real"
}

verify_devices() {
    local cache_device volume_device dest_device cache_target cache_root volume_root
    local dest_real volume_real relative expected_root
    mountpoint -q "$source_path" || fail "$source_path is not a mount point"
    verify_destination_on_shockwave
    cache_device="$(findmnt -n -o MAJ:MIN -T "$source_path")"
    volume_device="$(findmnt -n -o MAJ:MIN -T "$volume_path")"
    dest_device="$(findmnt -n -o MAJ:MIN -T "$dest_path")"
    [[ "$cache_device" == "$volume_device" && "$dest_device" == "$volume_device" ]] \
        || fail "cache/destination device does not match Shockwave ($cache_device/$dest_device vs $volume_device)"

    cache_target="$(findmnt -n -o TARGET --mountpoint "$source_path")"
    [[ "$cache_target" == "$source_path" ]] || fail "cache mount target is not exact: $cache_target"
    cache_root="$(findmnt -n -o FSROOT --mountpoint "$source_path")"
    volume_root="$(findmnt -n -o FSROOT --mountpoint "$volume_path")"
    dest_real="$(realpath -e "$dest_path")"
    volume_real="$(realpath -e "$volume_path")"
    [[ "$dest_real" == "$volume_real/"* ]] || fail "$dest_path is outside the Shockwave mount"
    relative="${dest_real#"$volume_real"}"
    if [[ "$volume_root" == / ]]; then
        expected_root="$relative"
    else
        expected_root="${volume_root%/}$relative"
    fi
    [[ "$cache_root" == "$expected_root" ]] \
        || fail "cache bind root $cache_root does not match Shockwave subtree $expected_root"
}

require_services_active() {
    local service
    for service in "${services[@]}"; do
        systemctl is-active --quiet "$service" \
            || fail "$service must be active before cutover"
    done
}

restore_pre_cutover_services() {
    sudo systemctl start "${services[@]}" || return 1
    wait_for_idle_online || return 1
    services_may_be_stopped=0
    restore_routes || return 1
}

record_tree_shape() {
    local root="$1"
    sudo find "$root" -xdev -printf '%y %s\n' | awk -v root="$root" \
        '{count[$1]++; bytes[$1]+=$2} END {for (type in count) printf "root=%s type=%s count=%d logical_bytes=%.0f\n", root, type, count[type], bytes[type]}'
    sudo du -s -B1 "$root"
}

verify_representative_hashes() {
    local rel source_hash dest_hash index
    local -a representatives=()
    mapfile -d '' -t representatives < <(
        sudo find "$source_path" -xdev -type f -size "$representative_find_size" -printf '%P\0'
    )
    [[ "${#representatives[@]}" -ge "$representative_required" ]] \
        || fail "fewer than $representative_required representative cache files were available to hash"
    for ((index = 0; index < representative_required; index++)); do
        rel="${representatives[$index]}"
        source_hash="$(sudo sha256sum -- "$source_path/$rel" | awk '{print $1}')"
        dest_hash="$(sudo sha256sum -- "$dest_path/$rel" | awk '{print $1}')"
        [[ "$source_hash" == "$dest_hash" ]] || fail "hash mismatch: $rel"
        printf 'hash=%s path=%s\n' "$source_hash" "$rel"
    done
}

restore_optional_file() {
    local existed="$1" backup="$2" target="$3" mode="$4"
    if [[ "$existed" == 1 ]]; then
        sudo install -o root -g root -m "$mode" "$backup" "$target"
    else
        sudo rm -f -- "$target"
    fi
}

rollback_host() {
    local config_status=0
    drain_and_stop_for_rollback || return 1

    [[ -f "$state_file.fstab" ]] \
        && sudo install -o root -g root -m 0644 "$state_file.fstab" "$fstab_path" \
        || config_status=1
    [[ -f "$state_file.unit-1" ]] \
        && sudo install -o root -g root -m 0644 "$state_file.unit-1" "$systemd_dir/cassy-actions-runner.service" \
        || config_status=1
    [[ -f "$state_file.unit-2" ]] \
        && sudo install -o root -g root -m 0644 "$state_file.unit-2" "$systemd_dir/cassy-actions-runner-2.service" \
        || config_status=1
    restore_optional_file "$pruner_existed" "$state_file.pruner" "$runner_root/prune-cache.sh" 0755 \
        || config_status=1
    restore_optional_file "$mount_guard_existed" "$state_file.mount-guard" "$runner_root/check-cache-mount.sh" 0755 \
        || config_status=1
    if [[ "$config_status" != 0 ]]; then
        printf 'rollback could not restore all configuration; services remain stopped and routes remain disabled\n' >&2
        return 1
    fi

    if mountpoint -q "$source_path" && ! sudo umount "$source_path"; then
        printf 'rollback could not unmount %s; services remain stopped and routes remain disabled\n' "$source_path" >&2
        return 1
    fi
    if [[ "$renamed" == 1 ]]; then
        [[ -d "$rollback_path" ]] || fail "rollback directory is absent: $rollback_path"
        sudo rmdir "$source_path" || return 1
        sudo mv "$rollback_path" "$source_path" || return 1
        renamed=0
    fi
    sudo systemctl daemon-reload || return 1
    sudo systemctl start "${services[@]}" || return 1
    wait_for_idle_online || {
        printf 'rollback restored root data but runners did not return online; routes remain disabled\n' >&2
        return 1
    }
    restore_routes || {
        printf 'rollback restored host but exact route restoration failed\n' >&2
        return 1
    }
    services_may_be_stopped=0
}

on_cutover_error() {
    local status=$? rollback_status
    trap - ERR
    set +e
    printf 'cutover failed (status %s); restoring original state\n' "$status" >&2
    if [[ "$host_mutated" == 1 ]]; then
        rollback_host
        rollback_status=$?
    elif [[ "$services_may_be_stopped" == 1 ]]; then
        restore_pre_cutover_services
        rollback_status=$?
    else
        restore_routes
        rollback_status=$?
    fi
    set -e
    if [[ "$rollback_status" == 0 ]]; then
        printf 'automatic rollback restored original host and route state\n' >&2
        exit "$status"
    fi
    printf 'automatic rollback incomplete; keep routes disabled and recover with state file: %s\n' "$state_file" >&2
    exit 70
}

inject_test_failure() {
    local stage="$1"
    if [[ "$testing" == 1 && "${CASSY_ACTIONS_CUTOVER_TEST_FAIL_AFTER:-}" == "$stage" ]]; then
        printf 'error: injected failure after %s\n' "$stage" >&2
        return 97
    fi
}

cutover() {
    local stamp fstab_candidate zero_diff proof_file
    command -v gh >/dev/null || fail 'gh is required'
    command -v rsync >/dev/null || fail 'rsync is required'
    sudo -n true || fail 'passwordless or pre-authorized sudo is required'
    gh api user >/dev/null 2>&1 || fail 'gh API authentication is required'
    [[ -d "$source_path" && -d "$dest_path" ]] || fail 'exact source and destination must already exist'
    mountpoint -q "$volume_path" || fail 'Shockwave is not mounted'
    verify_destination_on_shockwave
    require_services_active

    if mountpoint -q "$source_path"; then
        verify_devices
        printf 'already cut over: %s resolves to exact subtree %s\n' "$source_path" "$dest_path"
        return 0
    fi

    install -d -m 0750 "$artifact_root"
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    state_file="$artifact_root/operator-cutover-$stamp.env"
    rollback_path="$runner_root/cache.root-backup-$stamp"
    [[ ! -e "$rollback_path" ]] || fail "rollback target already exists: $rollback_path"

    original_pilot="$(route_get CASSY_SELF_HOSTED_PILOT)"
    original_merge_queue="$(route_get CASSY_MERGE_QUEUE_SELF_HOSTED)"
    original_release="$(route_get CASSY_RELEASE_SELF_HOSTED)"
    routes_captured=1
    [[ -e "$runner_root/prune-cache.sh" ]] && pruner_existed=1
    [[ -e "$runner_root/check-cache-mount.sh" ]] && mount_guard_existed=1
    printf 'ORIGINAL_PILOT=%q\nORIGINAL_MERGE_QUEUE=%q\nORIGINAL_RELEASE=%q\nROLLBACK_PATH=%q\n' \
        "$original_pilot" "$original_merge_queue" "$original_release" "$rollback_path" >"$state_file"
    printf 'PRUNER_EXISTED=%q\nMOUNT_GUARD_EXISTED=%q\n' \
        "$pruner_existed" "$mount_guard_existed" >>"$state_file"
    sudo cp -a "$fstab_path" "$state_file.fstab"
    sudo cp -a "$systemd_dir/cassy-actions-runner.service" "$state_file.unit-1"
    sudo cp -a "$systemd_dir/cassy-actions-runner-2.service" "$state_file.unit-2"
    [[ "$pruner_existed" == 1 ]] && sudo cp -a "$runner_root/prune-cache.sh" "$state_file.pruner"
    [[ "$mount_guard_existed" == 1 ]] && sudo cp -a "$runner_root/check-cache-mount.sh" "$state_file.mount-guard"
    ln -sfn "$(basename "$state_file")" "$artifact_root/operator-cutover-latest.env"
    trap on_cutover_error ERR

    disable_routes
    inject_test_failure route-disable
    runner_snapshot | tee "$state_file.runners-before"
    drain_and_stop_for_cutover

    verify_destination_on_shockwave
    sudo rsync -aHAX --numeric-ids --delete --info=stats2 \
        "$source_path/" "$dest_path/" | tee "$state_file.rsync"
    zero_diff="$state_file.zero-diff"
    sudo rsync -aHAXn --numeric-ids --delete --itemize-changes \
        "$source_path/" "$dest_path/" >"$zero_diff"
    [[ ! -s "$zero_diff" ]] || fail 'final rsync verification is not zero-diff'
    {
        record_tree_shape "$source_path"
        record_tree_shape "$dest_path"
        verify_representative_hashes
    } | tee "$state_file.copy-proof"
    inject_test_failure pre-rename

    sudo mv "$source_path" "$rollback_path"
    renamed=1
    host_mutated=1
    sudo install -d -o "$runner_user" -g "$runner_user" -m 0750 "$source_path"
    inject_test_failure source-rename
    fstab_candidate="$state_file.fstab-candidate"
    awk -v target="$source_path" -v line="$fstab_line" \
        '$2 != target {print} END {print line}' "$fstab_path" >"$fstab_candidate"
    sudo install -o root -g root -m 0644 "$fstab_candidate" "$fstab_path"
    sudo mount "$source_path"
    verify_devices
    inject_test_failure mount

    sudo install -o root -g root -m 0755 "$repo_root/scripts/prune-cassy-actions-cache.sh" "$runner_root/prune-cache.sh"
    sudo install -o root -g root -m 0755 "$repo_root/scripts/check-cassy-actions-cache-mount.sh" "$runner_root/check-cache-mount.sh"
    sudo install -o root -g root -m 0644 "$repo_root/ops/systemd/cassy-actions-runner.service" "$systemd_dir/cassy-actions-runner.service"
    sudo install -o root -g root -m 0644 "$repo_root/ops/systemd/cassy-actions-runner-2.service" "$systemd_dir/cassy-actions-runner-2.service"
    sudo systemctl daemon-reload
    sudo systemd-analyze verify "$systemd_dir/cassy-actions-runner.service" "$systemd_dir/cassy-actions-runner-2.service"
    sudo systemctl start "${services[@]}"
    services_may_be_stopped=0
    inject_test_failure service-restart
    wait_for_idle_online || fail 'both runners did not return online and idle within 60 seconds'

    proof_file="$source_path/cargo-target/.cas-a352-shockwave-proof-$stamp"
    sudo -u "$runner_user" touch "$proof_file"
    verify_devices
    {
        date --iso-8601=seconds
        findmnt -T "$proof_file" -o TARGET,SOURCE,FSTYPE,OPTIONS,SIZE,USED,AVAIL
        systemctl show "${services[@]}" -p Id -p ActiveState -p SubState -p MainPID -p Result --no-pager
        runner_snapshot
        printf 'rollback_path=%s\n' "$rollback_path"
    } | tee "$state_file.after-proof"
    restore_routes
    trap - ERR
    host_mutated=0
    printf 'cutover complete; keep rollback data: %s\nstate: %s\n' "$rollback_path" "$state_file"
}

rollback() {
    state_file="${1:?rollback requires the operator-cutover state file path}"
    [[ -f "$state_file" ]] || fail "state file not found: $state_file"
    # shellcheck disable=SC1090 -- this is a script-generated, operator-selected state file.
    source "$state_file"
    original_pilot="$ORIGINAL_PILOT"
    original_merge_queue="$ORIGINAL_MERGE_QUEUE"
    original_release="$ORIGINAL_RELEASE"
    rollback_path="$ROLLBACK_PATH"
    pruner_existed="${PRUNER_EXISTED:-0}"
    mount_guard_existed="${MOUNT_GUARD_EXISTED:-0}"
    routes_captured=1
    [[ "$rollback_path" == "$runner_root"/cache.root-backup-* && -d "$rollback_path" ]] \
        || fail 'validated rollback directory is absent'
    if ! disable_routes; then
        restore_routes || true
        fail 'rollback aborted before service stop because routing could not be disabled'
    fi
    renamed=1
    host_mutated=1
    if ! rollback_host; then
        fail "rollback incomplete; keep routes disabled and recover with state file: $state_file"
    fi
    host_mutated=0
    printf 'rollback complete from %s\n' "$rollback_path"
}

case "${1:-}" in
    cutover) cutover ;;
    rollback) rollback "${2:-}" ;;
    *) fail "usage: $0 cutover | rollback <operator-cutover-state.env>" ;;
esac
