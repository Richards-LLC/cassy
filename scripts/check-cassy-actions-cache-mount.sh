#!/usr/bin/env bash
# Refuse to start a persistent runner when its cache fell back to host root.
set -euo pipefail

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

production_cache=/var/lib/cassy-actions/cache
production_volume=/mnt/shockwave
cache_root="$(realpath -m -- "${CASSY_ACTIONS_CACHE_ROOT:-$production_cache}")"
volume_root="$(realpath -m -- "${CASSY_ACTIONS_VOLUME_ROOT:-$production_volume}")"
findmnt_bin="${CASSY_ACTIONS_FINDMNT_BIN:-/usr/bin/findmnt}"
mountpoint_bin="${CASSY_ACTIONS_MOUNTPOINT_BIN:-/usr/bin/mountpoint}"

if [[ "$cache_root" != "$production_cache" || "$volume_root" != "$production_volume" ]]; then
    [[ "${CASSY_ACTIONS_ALLOW_TEST_ROOT:-}" == 1 ]] \
        || fail "refusing alternate cache/volume roots outside a test fixture"
fi
[[ -x "$findmnt_bin" && -x "$mountpoint_bin" ]] || fail "findmnt and mountpoint are required"
"$mountpoint_bin" -q "$volume_root" || fail "dedicated cache volume is not mounted: $volume_root"
"$mountpoint_bin" -q "$cache_root" || fail "runner cache is not a mount point: $cache_root"

volume_device="$("$findmnt_bin" -n -o MAJ:MIN -T "$volume_root")"
cache_device="$("$findmnt_bin" -n -o MAJ:MIN -T "$cache_root")"
[[ -n "$volume_device" && "$cache_device" == "$volume_device" ]] \
    || fail "runner cache device $cache_device does not match dedicated volume device $volume_device"

printf 'runner cache mount verified: cache=%s volume=%s device=%s\n' \
    "$cache_root" "$volume_root" "$cache_device"
