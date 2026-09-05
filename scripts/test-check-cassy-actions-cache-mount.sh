#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-cassy-actions-cache-mount.sh"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

mkdir -p "$fixture_root/bin" "$fixture_root/cache" "$fixture_root/volume" \
    "$fixture_root/backing"

cat >"$fixture_root/bin/mountpoint" <<'EOF'
#!/usr/bin/env bash
[[ "${TEST_MOUNTPOINTS:-both}" == both ]]
EOF

cat >"$fixture_root/bin/findmnt" <<'EOF'
#!/usr/bin/env bash
field=""
target=""
first_only=0
while (($#)); do
    case "$1" in
        -f|--first-only) first_only=1; shift ;;
        -o) field="$2"; shift 2 ;;
        -T) target="$2"; shift 2 ;;
        *) shift ;;
    esac
done
case "$field:$target" in
    MAJ:MIN:*cache) value="${TEST_CACHE_DEVICE:-259:1}" ;;
    MAJ:MIN:*) value="${TEST_VOLUME_DEVICE:-259:1}" ;;
    FSROOT:*cache) value="${TEST_CACHE_FSROOT:-/home/.cassy-actions-cache}" ;;
    *) exit 2 ;;
esac
printf '%s\n' "$value"
if [[ "${TEST_DUPLICATE_ROWS:-}" == 1 && "$first_only" == 0 ]]; then
    printf '%s\n' "$value"
fi
EOF
chmod +x "$fixture_root/bin/mountpoint" "$fixture_root/bin/findmnt"

run_guard() {
    CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
    CASSY_ACTIONS_CACHE_ROOT="$fixture_root/cache" \
    CASSY_ACTIONS_VOLUME_ROOT="$fixture_root/volume" \
    CASSY_ACTIONS_EXPECTED_FSROOT=/home/.cassy-actions-cache \
    CASSY_ACTIONS_MOUNTPOINT_BIN="$fixture_root/bin/mountpoint" \
    CASSY_ACTIONS_FINDMNT_BIN="$fixture_root/bin/findmnt" \
        "$guard"
}

run_guard >/dev/null

# systemd's service mount namespace can expose the same underlying mount more
# than once. The guard must select one record instead of treating duplicate
# identical rows as a different device or FSROOT.
TEST_DUPLICATE_ROWS=1 run_guard >/dev/null

if TEST_CACHE_FSROOT=/home/wrong-but-same-device run_guard >/dev/null 2>&1; then
    printf 'FAIL same-device wrong cache subtree was accepted\n' >&2
    exit 1
fi

if TEST_CACHE_DEVICE=259:2 run_guard >/dev/null 2>&1; then
    printf 'FAIL wrong cache device was accepted\n' >&2
    exit 1
fi

if TEST_MOUNTPOINTS=missing run_guard >/dev/null 2>&1; then
    printf 'FAIL missing mount was accepted\n' >&2
    exit 1
fi

printf 'ok   cache mount guard requires the exact Shockwave device and FSROOT\n'
