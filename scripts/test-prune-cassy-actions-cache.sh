#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
pruner="$script_dir/prune-cassy-actions-cache.sh"
fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

cache_root="$fixture_root/cache"
cgroup_root="$fixture_root/cgroup"
proc_root="$fixture_root/proc"
mkdir -p "$cache_root" "$cgroup_root/slot1" "$cgroup_root/slot2" "$proc_root/101" "$proc_root/102"
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

run_pruner() {
    CASSY_ACTIONS_ALLOW_TEST_ROOT=1 \
    CASSY_ACTIONS_CACHE_ROOT="$cache_root" \
    CASSY_ACTIONS_CGROUP_ROOT="$cgroup_root" \
    CASSY_ACTIONS_PROC_ROOT="$proc_root" \
    CASSY_ACTIONS_SYSTEMCTL_BIN="$fixture_root/systemctl" \
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
