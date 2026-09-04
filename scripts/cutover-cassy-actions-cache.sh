#!/usr/bin/env bash
# Operator-run, reversible migration of the soundwave runner cache to Shockwave.
# Run as the logged-in GitHub operator; this script uses sudo only for host writes.
set -euo pipefail

repo=Richards-LLC/cassy
source_path=/var/lib/cassy-actions/cache
dest_path=/mnt/shockwave/home/pippenz/cassy-actions/cache
volume_path=/mnt/shockwave
artifact_root="${CASSY_ACTIONS_CUTOVER_ARTIFACT_ROOT:-/home/pippenz/.cas/artifacts/cas-a352}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
services=(cassy-actions-runner.service cassy-actions-runner-2.service)
route_vars=(CASSY_SELF_HOSTED_PILOT CASSY_MERGE_QUEUE_SELF_HOSTED CASSY_RELEASE_SELF_HOSTED)
fstab_line="$dest_path $source_path none bind,x-systemd.requires-mounts-for=$volume_path 0 0"
state_file=
rollback_path=
renamed=0

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

route_get() {
    gh variable get "$1" --repo "$repo" 2>/dev/null || printf '<unset>\n'
}

route_set() {
    local name="$1" value="$2"
    if [[ "$value" == '<unset>' ]]; then
        gh variable delete "$name" --repo "$repo" 2>/dev/null || true
    else
        gh variable set "$name" --repo "$repo" --body "$value"
    fi
}

restore_routes() {
    [[ -n "${original_pilot+x}" ]] || return 0
    route_set CASSY_SELF_HOSTED_PILOT "$original_pilot"
    route_set CASSY_MERGE_QUEUE_SELF_HOSTED "$original_merge_queue"
    route_set CASSY_RELEASE_SELF_HOSTED "$original_release"
}

disable_routes() {
    local name
    for name in "${route_vars[@]}"; do
        gh variable set "$name" --repo "$repo" --body disabled
    done
}

runner_snapshot() {
    gh api orgs/Richards-LLC/actions/runners --jq \
        '.runners[] | select(.name == "soundwave-cas-ci" or .name == "soundwave-cas-ci-2") | {name,status,busy}'
}

wait_for_idle_online() {
    local attempt count
    for attempt in {1..60}; do
        count="$(gh api orgs/Richards-LLC/actions/runners --jq \
            '[.runners[] | select((.name == "soundwave-cas-ci" or .name == "soundwave-cas-ci-2") and .status == "online" and .busy == false)] | length')"
        [[ "$count" == 2 ]] && return 0
        sleep 1
    done
    return 1
}

prove_no_job_process() {
    if pgrep -u cassy-actions -f 'Runner.Worker' >/dev/null; then
        pgrep -a -u cassy-actions -f 'Runner.Worker' >&2
        fail 'a runner job process is active'
    fi
}

verify_devices() {
    local cache_device volume_device
    mountpoint -q "$source_path" || fail "$source_path is not a mount point"
    mountpoint -q "$volume_path" || fail "$volume_path is not a mount point"
    cache_device="$(findmnt -n -o MAJ:MIN -T "$source_path")"
    volume_device="$(findmnt -n -o MAJ:MIN -T "$volume_path")"
    [[ "$cache_device" == "$volume_device" ]] \
        || fail "cache device $cache_device differs from Shockwave device $volume_device"
}

record_tree_shape() {
    local root="$1"
    sudo find "$root" -xdev -printf '%y %s\n' | awk -v root="$root" \
        '{count[$1]++; bytes[$1]+=$2} END {for (type in count) printf "root=%s type=%s count=%d logical_bytes=%.0f\n", root, type, count[type], bytes[type]}'
    sudo du -s -B1 "$root"
}

verify_representative_hashes() {
    local rel source_hash dest_hash checked=0
    while IFS= read -r rel; do
        source_hash="$(sudo sha256sum "$source_path/$rel" | awk '{print $1}')"
        dest_hash="$(sudo sha256sum "$dest_path/$rel" | awk '{print $1}')"
        [[ "$source_hash" == "$dest_hash" ]] || fail "hash mismatch: $rel"
        printf 'hash=%s path=%s\n' "$source_hash" "$rel"
        checked=$((checked + 1))
    done < <(sudo find "$source_path" -xdev -type f -size +100M -printf '%P\n' | head -3)
    [[ "$checked" -eq 3 ]] || fail 'fewer than three representative cache files were available to hash'
}

rollback_host() {
    trap - ERR
    set +e
    sudo systemctl stop "${services[@]}"
    mountpoint -q "$source_path" && sudo umount "$source_path"
    [[ -n "$state_file" && -f "$state_file.fstab" ]] && sudo install -o root -g root -m 0644 "$state_file.fstab" /etc/fstab
    [[ -n "$state_file" && -f "$state_file.unit-1" ]] && sudo install -o root -g root -m 0644 "$state_file.unit-1" /etc/systemd/system/cassy-actions-runner.service
    [[ -n "$state_file" && -f "$state_file.unit-2" ]] && sudo install -o root -g root -m 0644 "$state_file.unit-2" /etc/systemd/system/cassy-actions-runner-2.service
    if [[ "$renamed" -eq 1 && -d "$rollback_path" ]]; then
        sudo rmdir "$source_path" 2>/dev/null || true
        [[ -e "$source_path" ]] || sudo mv "$rollback_path" "$source_path"
    fi
    sudo systemctl daemon-reload
    sudo systemctl start "${services[@]}"
    restore_routes
    set -e
}

on_cutover_error() {
    local status=$?
    printf 'cutover failed (status %s); restoring original host and route state\n' "$status" >&2
    rollback_host
    exit "$status"
}

cutover() {
    local stamp fstab_candidate zero_diff proof_file
    command -v gh >/dev/null || fail 'gh is required'
    command -v rsync >/dev/null || fail 'rsync is required'
    sudo -n true || fail 'passwordless or pre-authorized sudo is required'
    gh api user >/dev/null 2>&1 || fail 'gh API authentication is required'
    [[ -d "$source_path" && -d "$dest_path" ]] || fail 'exact source and destination must already exist'
    mountpoint -q "$volume_path" || fail 'Shockwave is not mounted'

    if mountpoint -q "$source_path"; then
        verify_devices
        printf 'already cut over: %s resolves to %s\n' "$source_path" "$volume_path"
        return 0
    fi

    install -d -m 0750 "$artifact_root"
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    state_file="$artifact_root/operator-cutover-$stamp.env"
    rollback_path="/var/lib/cassy-actions/cache.root-backup-$stamp"
    [[ ! -e "$rollback_path" ]] || fail "rollback target already exists: $rollback_path"

    original_pilot="$(route_get CASSY_SELF_HOSTED_PILOT)"
    original_merge_queue="$(route_get CASSY_MERGE_QUEUE_SELF_HOSTED)"
    original_release="$(route_get CASSY_RELEASE_SELF_HOSTED)"
    printf 'ORIGINAL_PILOT=%q\nORIGINAL_MERGE_QUEUE=%q\nORIGINAL_RELEASE=%q\nROLLBACK_PATH=%q\n' \
        "$original_pilot" "$original_merge_queue" "$original_release" "$rollback_path" >"$state_file"
    sudo cp -a /etc/fstab "$state_file.fstab"
    sudo cp -a /etc/systemd/system/cassy-actions-runner.service "$state_file.unit-1"
    sudo cp -a /etc/systemd/system/cassy-actions-runner-2.service "$state_file.unit-2"
    ln -sfn "$(basename "$state_file")" "$artifact_root/operator-cutover-latest.env"
    trap on_cutover_error ERR

    disable_routes
    runner_snapshot | tee "$state_file.runners-before"
    if runner_snapshot | grep -q '"busy":true'; then
        fail 'a runner became busy after routing was disabled'
    fi
    sudo systemctl stop "${services[@]}"
    prove_no_job_process

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

    sudo mv "$source_path" "$rollback_path"
    renamed=1
    sudo install -d -o cassy-actions -g cassy-actions -m 0750 "$source_path"
    fstab_candidate="$state_file.fstab-candidate"
    awk -v target="$source_path" -v line="$fstab_line" \
        '$2 != target {print} END {print line}' /etc/fstab >"$fstab_candidate"
    sudo install -o root -g root -m 0644 "$fstab_candidate" /etc/fstab
    sudo mount -a
    verify_devices

    sudo install -o root -g root -m 0755 "$repo_root/scripts/prune-cassy-actions-cache.sh" /var/lib/cassy-actions/prune-cache.sh
    sudo install -o root -g root -m 0755 "$repo_root/scripts/check-cassy-actions-cache-mount.sh" /var/lib/cassy-actions/check-cache-mount.sh
    sudo install -o root -g root -m 0644 "$repo_root/ops/systemd/cassy-actions-runner.service" /etc/systemd/system/cassy-actions-runner.service
    sudo install -o root -g root -m 0644 "$repo_root/ops/systemd/cassy-actions-runner-2.service" /etc/systemd/system/cassy-actions-runner-2.service
    sudo systemctl daemon-reload
    sudo systemd-analyze verify /etc/systemd/system/cassy-actions-runner.service /etc/systemd/system/cassy-actions-runner-2.service
    sudo systemctl start "${services[@]}"
    wait_for_idle_online || fail 'both runners did not return online and idle within 60 seconds'

    proof_file="$source_path/cargo-target/.cas-a352-shockwave-proof-$stamp"
    sudo -u cassy-actions touch "$proof_file"
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
    [[ "$rollback_path" == /var/lib/cassy-actions/cache.root-backup-* && -d "$rollback_path" ]] \
        || fail 'validated rollback directory is absent'
    disable_routes
    renamed=1
    rollback_host
    printf 'rollback complete from %s\n' "$rollback_path"
}

case "${1:-}" in
    cutover) cutover ;;
    rollback) rollback "${2:-}" ;;
    *) fail "usage: $0 cutover | rollback <operator-cutover-state.env>" ;;
esac
