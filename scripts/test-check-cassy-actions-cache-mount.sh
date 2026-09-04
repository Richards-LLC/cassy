#!/usr/bin/env bash
# Behavioral regression test for the runner cache mount fail-closed guard.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-cassy-actions-cache-mount.sh"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

mkdir -p "$fixture_root/bin" "$fixture_root/cache" "$fixture_root/volume"
printf '#!/usr/bin/env bash\nexit 0\n' >"$fixture_root/bin/mountpoint"
printf '#!/usr/bin/env bash\nprintf "%%s\\n" "${TEST_DEVICE:?}"\n' >"$fixture_root/bin/findmnt"
chmod +x "$fixture_root/bin/mountpoint" "$fixture_root/bin/findmnt"

CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
CASSY_ACTIONS_CACHE_ROOT="$fixture_root/cache" \
CASSY_ACTIONS_VOLUME_ROOT="$fixture_root/volume" \
CASSY_ACTIONS_MOUNTPOINT_BIN="$fixture_root/bin/mountpoint" \
CASSY_ACTIONS_FINDMNT_BIN="$fixture_root/bin/findmnt" \
TEST_DEVICE=8:1 "$guard" >/dev/null

printf '#!/usr/bin/env bash\ncase "$*" in *cache*) echo 8:2 ;; *) echo 8:1 ;; esac\n' >"$fixture_root/bin/findmnt"
chmod +x "$fixture_root/bin/findmnt"
if CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
    CASSY_ACTIONS_CACHE_ROOT="$fixture_root/cache" \
    CASSY_ACTIONS_VOLUME_ROOT="$fixture_root/volume" \
    CASSY_ACTIONS_MOUNTPOINT_BIN="$fixture_root/bin/mountpoint" \
    CASSY_ACTIONS_FINDMNT_BIN="$fixture_root/bin/findmnt" \
    "$guard" >/dev/null 2>&1; then
    printf 'FAIL cache on a different device was accepted\n' >&2
    exit 1
fi

printf 'ok   cache mount must resolve to the dedicated volume device\n'
