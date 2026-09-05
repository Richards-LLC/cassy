#!/usr/bin/env bash
# Refuse to start a persistent runner unless its cache is the expected
# Shockwave-backed bind mount, not merely another directory on the same device.
set -euo pipefail

fail() {
    printf 'error: %s\n' "$1" >&2
    exit 1
}

production_cache=/var/lib/cassy-actions/cache
production_volume=/mnt/shockwave
production_fsroot=/home/.cassy-actions-cache
cache_root="$(realpath -m -- "${CASSY_ACTIONS_CACHE_ROOT:-$production_cache}")"
volume_root="$(realpath -m -- "${CASSY_ACTIONS_VOLUME_ROOT:-$production_volume}")"
expected_fsroot="${CASSY_ACTIONS_EXPECTED_FSROOT:-$production_fsroot}"
findmnt_bin="${CASSY_ACTIONS_FINDMNT_BIN:-/usr/bin/findmnt}"
mountpoint_bin="${CASSY_ACTIONS_MOUNTPOINT_BIN:-/usr/bin/mountpoint}"

if [[ "$cache_root" != "$production_cache" || "$volume_root" != "$production_volume" ||
      "$expected_fsroot" != "$production_fsroot" ]]; then
    [[ "${CASSY_ACTIONS_ALLOW_TEST_ROOT:-}" == 1 ]] ||
        fail 'alternate cache, volume, or FSROOT is allowed only in a test fixture'
fi
[[ -x "$findmnt_bin" && -x "$mountpoint_bin" ]] || fail 'findmnt and mountpoint are required'
"$mountpoint_bin" -q "$volume_root" || fail "dedicated cache volume is not mounted: $volume_root"
"$mountpoint_bin" -q "$cache_root" || fail "runner cache is not a mount point: $cache_root"

volume_device="$("$findmnt_bin" -f -n -o MAJ:MIN -T "$volume_root")" ||
    fail "cannot resolve dedicated volume device: $volume_root"
cache_device="$("$findmnt_bin" -f -n -o MAJ:MIN -T "$cache_root")" ||
    fail "cannot resolve runner cache device: $cache_root"
cache_fsroot="$("$findmnt_bin" -f -n -o FSROOT -T "$cache_root")" ||
    fail "cannot resolve runner cache FSROOT: $cache_root"

[[ -n "$volume_device" && "$cache_device" == "$volume_device" ]] ||
    fail "runner cache device $cache_device does not match Shockwave device $volume_device"
[[ "$cache_fsroot" == "$expected_fsroot" ]] ||
    fail "runner cache FSROOT $cache_fsroot does not match expected $expected_fsroot"

printf 'runner cache mount verified: cache=%s fsroot=%s volume=%s device=%s\n' \
    "$cache_root" "$cache_fsroot" "$volume_root" "$cache_device"
