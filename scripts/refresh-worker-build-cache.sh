#!/usr/bin/env bash
# Build and atomically publish a quiescent Cargo target baseline for newly
# created factory worktrees. Run this after merging an integration/epic branch.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cas_dir="${CAS_ROOT:-${repo_root}/.cas}"
cache_root="${cas_dir}/build-cache"
snapshots_dir="${cache_root}/snapshots"
commit="$(git -C "${repo_root}" rev-parse --short=12 HEAD)"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
snapshot_name="target-${commit}-${stamp}-$$"
snapshot_dir="${snapshots_dir}/${snapshot_name}"
pointer_next="${cache_root}/current.next.$$"
cargo_bin="${CARGO:-cargo}"
if [[ $# -gt 0 ]]; then
    cargo_args=("$@")
else
    cargo_args=(--workspace --lib --tests)
fi

mkdir -p "${snapshots_dir}"

cleanup_failed_refresh() {
    status=$?
    trap - EXIT
    if [[ ${status} -ne 0 ]]; then
        rm -rf -- "${snapshot_dir}"
        rm -f -- "${pointer_next}"
    fi
    exit "${status}"
}
trap cleanup_failed_refresh EXIT

echo "Building quiescent worker target snapshot: ${snapshot_dir}"
echo "+ CARGO_TARGET_DIR=${snapshot_dir} ${cargo_bin} check ${cargo_args[*]}"
(
    cd "${repo_root}"
    CARGO_TARGET_DIR="${snapshot_dir}" "${cargo_bin}" check "${cargo_args[@]}"
)

# The snapshot is never written again. Publishing a small pointer file through
# rename keeps worktree provisioning on either the complete old snapshot or
# the complete new snapshot, never a half-built directory.
printf '%s\n' "${snapshot_name}" > "${pointer_next}"
mv -f -- "${pointer_next}" "${cache_root}/current"
trap - EXIT

echo "Published worker target baseline: ${snapshot_name}"
echo "Old snapshots remain valid for in-flight seeders; remove them only during a quiescent maintenance window."
