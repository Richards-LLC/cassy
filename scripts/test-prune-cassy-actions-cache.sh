#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pruner="$script_dir/prune-cassy-actions-cache.sh"
mount_guard="$script_dir/check-cassy-actions-cache-mount.sh"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

cache_root="$fixture_root/cache"
cgroup_root="$fixture_root/cgroup"
proc_root="$fixture_root/proc"
mkdir -p "$cache_root" "$cgroup_root/slot1" "$cgroup_root/slot2" "$proc_root/101" "$proc_root/102"
mkdir -p "$fixture_root/volume"
printf '101\n' >"$cgroup_root/slot1/cgroup.procs"
printf '102\n' >"$cgroup_root/slot2/cgroup.procs"
printf 'Runner.Listener\n' >"$proc_root/101/comm"
printf 'Runner.Listener\n' >"$proc_root/102/comm"

cat >"$fixture_root/systemctl" <<'EOF'
#!/usr/bin/env bash
case "$*" in
    *cassy-actions-runner-2.service*) printf '/slot2\n' ;;
    *cassy-actions-runner.service*) printf '/slot1\n' ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$fixture_root/systemctl"

cat >"$fixture_root/mountpoint" <<'EOF'
#!/usr/bin/env bash
[[ "${TEST_MOUNTPOINTS:-both}" == both ]]
EOF

cat >"$fixture_root/findmnt" <<'EOF'
#!/usr/bin/env bash
field=""
target=""
recursive=0
while (($#)); do
    case "$1" in
        -R|--submounts) recursive=1; shift ;;
        -o) field="$2"; shift 2 ;;
        -T) target="$2"; shift 2 ;;
        *) shift ;;
    esac
done
case "$field:$target" in
    TARGET:*cache*)
        if (( recursive )) && [[ "${TEST_MOUNT_ENUM_FAIL:-}" == 1 ]]; then
            exit 3
        fi
        if (( recursive )) && [[ "${TEST_DESCENDANT_NESTED_MOUNT:-}" == 1 && "$target" == */debug ]]; then
            printf '%s\n%s/subdir\n' "$CASSY_ACTIONS_CACHE_ROOT" "$target"
        elif [[ "${TEST_DELETE_NESTED_MOUNT:-}" == 1 && "$target" == */debug ]]; then
            printf '%s/cargo-target/debug\n' "$CASSY_ACTIONS_CACHE_ROOT"
        else
            printf '%s\n' "$CASSY_ACTIONS_CACHE_ROOT"
        fi
        ;;
    MAJ:MIN:*cache*) printf '%s\n' "${TEST_CACHE_DEVICE:-259:1}" ;;
    MAJ:MIN:*) printf '%s\n' "${TEST_VOLUME_DEVICE:-259:1}" ;;
    FSROOT:*cache*) printf '%s\n' "${TEST_CACHE_FSROOT:-/home/.cassy-actions-cache}" ;;
    *) exit 2 ;;
esac
EOF
chmod +x "$fixture_root/mountpoint" "$fixture_root/findmnt"

run_pruner() {
    CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
    CASSY_ACTIONS_CACHE_ROOT="$cache_root" \
    CASSY_ACTIONS_CGROUP_ROOT="$cgroup_root" \
    CASSY_ACTIONS_PROC_ROOT="$proc_root" \
    CASSY_ACTIONS_SYSTEMCTL_BIN="$fixture_root/systemctl" \
    CASSY_ACTIONS_MOUNT_GUARD_BIN="$mount_guard" \
    CASSY_ACTIONS_VOLUME_ROOT="$fixture_root/volume" \
    CASSY_ACTIONS_EXPECTED_FSROOT=/home/.cassy-actions-cache \
    CASSY_ACTIONS_MOUNTPOINT_BIN="$fixture_root/mountpoint" \
    CASSY_ACTIONS_FINDMNT_BIN="$fixture_root/findmnt" \
    CASSY_ACTIONS_TARGET_BUDGET_BYTES=32768 \
    CASSY_ACTIONS_SCCACHE_BUDGET_BYTES=8192 \
    CASSY_ACTIONS_SLOT_BUDGET_BYTES=60000 \
    CASSY_ACTIONS_CACHE_MAX_AGE_DAYS=7 \
        "$pruner" "$@"
}

for slot in '' '-2'; do
    mkdir -p "$cache_root/cargo-target$slot/debug/incremental/stale-session" \
        "$cache_root/cargo-target$slot/debug/deps" "$cache_root/sccache$slot"
    dd if=/dev/zero of="$cache_root/cargo-target$slot/debug/incremental/stale-session/object.o" bs=4096 count=1 status=none
    dd if=/dev/zero of="$cache_root/cargo-target$slot/debug/deps/libstale.rlib" bs=4096 count=1 status=none
    dd if=/dev/zero of="$cache_root/cargo-target$slot/debug/deps/libfresh.rlib" bs=4096 count=1 status=none
    touch -d '8 days ago' "$cache_root/cargo-target$slot/debug/incremental/stale-session" \
        "$cache_root/cargo-target$slot/debug/deps/libstale.rlib"
done

run_pruner --now >/dev/null
for slot in '' '-2'; do
    test ! -e "$cache_root/cargo-target$slot/debug/incremental/stale-session"
    test ! -e "$cache_root/cargo-target$slot/debug/deps/libstale.rlib"
    test -e "$cache_root/cargo-target$slot/debug/deps/libfresh.rlib"
done
printf 'ok   idle pruning removes stale incremental/deps data from both slots\n'

mkdir -p "$proc_root/103"
printf '103\n' >>"$cgroup_root/slot1/cgroup.procs"
printf 'Runner.Worker\n' >"$proc_root/103/comm"
busy_marker="$cache_root/cargo-target/debug/incremental/busy-marker"
mkdir -p "$busy_marker"
touch -d '8 days ago' "$busy_marker"
run_pruner --scheduled >/dev/null
test -d "$busy_marker"
if run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL forced prune accepted Runner.Worker without cargo/rustc\n' >&2
    exit 1
fi
test -d "$busy_marker"
printf 'ok   Runner.Worker blocks pruning even without cargo/rustc\n'

sed -i '/103/d' "$cgroup_root/slot1/cgroup.procs"
mv "$cgroup_root/slot2/cgroup.procs" "$cgroup_root/slot2/cgroup.procs.unreadable"
if run_pruner --check-idle >/dev/null 2>&1; then
    printf 'FAIL missing job-state file was treated as idle\n' >&2
    exit 1
fi
test -d "$busy_marker"
mv "$cgroup_root/slot2/cgroup.procs.unreadable" "$cgroup_root/slot2/cgroup.procs"
printf 'ok   missing or unreadable job state fails closed\n'

mount_sentinel="$cache_root/cargo-target/debug/incremental/mount-sentinel"
mkdir -p "$mount_sentinel"
touch -d '8 days ago' "$mount_sentinel"
if TEST_CACHE_FSROOT=/home/wrong-but-same-device run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL pruner accepted the wrong cache FSROOT\n' >&2
    exit 1
fi
test -d "$mount_sentinel"
if TEST_CACHE_DEVICE=259:2 run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL pruner accepted the wrong cache device\n' >&2
    exit 1
fi
test -d "$mount_sentinel"
printf 'ok   destructive modes recheck exact cache FSROOT and device\n'

budget_cache="$fixture_root/budget-cache"
cache_root="$budget_cache"
for slot in '' '-2'; do
    mkdir -p "$cache_root/cargo-target$slot/debug/deps" "$cache_root/sccache$slot"
    dd if=/dev/zero of="$cache_root/cargo-target$slot/debug/deps/current" bs=4096 count=16 status=none
    dd if=/dev/zero of="$cache_root/sccache$slot/current" bs=4096 count=2 status=none
done
run_pruner --now >/dev/null
for slot in '' '-2'; do
    test ! -e "$cache_root/cargo-target$slot/debug"
    combined=$(( $(du -s -B1 "$cache_root/cargo-target$slot" | awk '{print $1}') + $(du -s -B1 "$cache_root/sccache$slot" | awk '{print $1}') ))
    test "$combined" -le 60000
done
printf 'ok   actual target plus sccache bytes remain below the per-slot cap\n'

opaque_cache="$fixture_root/opaque-cache"
cache_root="$opaque_cache"
for slot in '' '-2'; do mkdir -p "$cache_root/cargo-target$slot" "$cache_root/sccache$slot"; done
dd if=/dev/zero of="$cache_root/cargo-target/operator-owned.bin" bs=4096 count=16 status=none
if run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL opaque over-budget data was accepted\n' >&2
    exit 1
fi
test -e "$cache_root/cargo-target/operator-owned.bin"
printf 'ok   pruning failure retains unknown over-budget data\n'

hostile_cache="$fixture_root/hostile-cache"
outside_profile="$fixture_root/outside-profile"
cache_root="$hostile_cache"
mkdir -p "$cache_root/cargo-target" "$cache_root/sccache" \
    "$cache_root/cargo-target-2" "$cache_root/sccache-2" \
    "$outside_profile/debug"
printf 'outside must survive\n' >"$outside_profile/debug/sentinel"
ln -s "$outside_profile" "$cache_root/cargo-target/x86_64-unknown-linux-gnu"
dd if=/dev/zero of="$cache_root/cargo-target/over-budget.bin" bs=4096 count=16 status=none
if run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL pruner accepted a profile below a symlink ancestor\n' >&2
    exit 1
fi
test -f "$outside_profile/debug/sentinel"
printf 'ok   symlink ancestors cannot redirect profile deletion outside the cache\n'

nested_cache="$fixture_root/nested-cache"
cache_root="$nested_cache"
mkdir -p "$cache_root/cargo-target/debug" "$cache_root/sccache" \
    "$cache_root/cargo-target-2" "$cache_root/sccache-2"
printf 'nested mount must survive\n' >"$cache_root/cargo-target/debug/nested-mount-sentinel"
dd if=/dev/zero of="$cache_root/cargo-target/debug/over-budget.bin" bs=4096 count=16 status=none
if TEST_DELETE_NESTED_MOUNT=1 run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL pruner accepted a profile on a nested mount\n' >&2
    exit 1
fi
test -f "$cache_root/cargo-target/debug/nested-mount-sentinel"
printf 'ok   profile deletion cannot cross onto a nested mount\n'

descendant_cache="$fixture_root/descendant-cache"
cache_root="$descendant_cache"
mkdir -p "$cache_root/cargo-target/debug/subdir" "$cache_root/sccache" \
    "$cache_root/cargo-target-2" "$cache_root/sccache-2"
printf 'nested descendant must survive\n' >"$cache_root/cargo-target/debug/subdir/sentinel"
dd if=/dev/zero of="$cache_root/cargo-target/debug/over-budget.bin" bs=4096 count=16 status=none
if TEST_DESCENDANT_NESTED_MOUNT=1 run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL recursive deletion accepted a nested mount below its root\n' >&2
    exit 1
fi
test -f "$cache_root/cargo-target/debug/subdir/sentinel"
printf 'ok   recursive profile deletion refuses descendant mounts before mutation\n'

enumeration_cache="$fixture_root/enumeration-cache"
cache_root="$enumeration_cache"
mkdir -p "$cache_root/cargo-target/debug" "$cache_root/sccache" \
    "$cache_root/cargo-target-2" "$cache_root/sccache-2"
printf 'enumeration failure must preserve this\n' >"$cache_root/cargo-target/debug/sentinel"
dd if=/dev/zero of="$cache_root/cargo-target/debug/over-budget.bin" bs=4096 count=16 status=none
if TEST_MOUNT_ENUM_FAIL=1 run_pruner --now >/dev/null 2>&1; then
    printf 'FAIL recursive deletion continued after mount enumeration failed\n' >&2
    exit 1
fi
test -f "$cache_root/cargo-target/debug/sentinel"
printf 'ok   mount enumeration failure refuses recursive deletion\n'

if CASSY_ACTIONS_ALLOW_TEST_ROOT=1 CASSY_ACTIONS_CACHE_ROOT="$cache_root" \
    CASSY_ACTIONS_CGROUP_ROOT="$cgroup_root" CASSY_ACTIONS_PROC_ROOT="$proc_root" \
    CASSY_ACTIONS_SYSTEMCTL_BIN="$fixture_root/systemctl" \
    CASSY_ACTIONS_TARGET_BUDGET_BYTES=52000000000 \
    CASSY_ACTIONS_SCCACHE_BUDGET_BYTES=8589934592 \
    CASSY_ACTIONS_SLOT_BUDGET_BYTES=60000000000 \
    "$pruner" --check-idle >/dev/null 2>&1; then
    printf 'FAIL configured target+sccache budget above 60,000,000,000 bytes was accepted\n' >&2
    exit 1
fi
printf 'ok   configured target+sccache budget is validated in bytes\n'
