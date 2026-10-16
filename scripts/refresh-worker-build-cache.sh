#!/usr/bin/env bash
# Build and atomically publish a quiescent Cargo target baseline for newly
# created factory worktrees. Run this after merging an integration/epic branch.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cas_dir="${CAS_ROOT:-${repo_root}/.cas}"

# cas-3f93: `[factory] scratch_root` moves the baseline to
# <scratch_root>/<project-key>/build-cache, beside the relocated worker
# targets it seeds. The key matches cas_types::factory_scratch::project_key:
# <project dir name, 48 chars of [A-Za-z0-9_-]>-<sha256 of the canonical store>.
configured_scratch_root() {
    command -v cas >/dev/null 2>&1 || return 0
    (cd "${repo_root}" && cas config get factory.scratch_root 2>/dev/null) | head -n 1 || true
}

project_key() {
    local store parent label hash
    store="$(cd "$1" 2>/dev/null && pwd -P || printf '%s' "$1")"
    parent="$(dirname -- "${store}")"
    label="project"
    if [[ "${parent}" != "/" && "${parent}" != "." ]]; then
        label="$(basename -- "${parent}")"
    fi
    label="${label:0:48}"
    label="${label//[^A-Za-z0-9_-]/-}"
    if command -v sha256sum >/dev/null 2>&1; then
        hash="$(printf '%s' "${store}" | sha256sum | cut -d' ' -f1)"
    else
        hash="$(printf '%s' "${store}" | shasum -a 256 | cut -d' ' -f1)"
    fi
    printf '%s-%s\n' "${label}" "${hash}"
}

scratch_root="$(configured_scratch_root)"
scratch_root="${scratch_root#"${scratch_root%%[![:space:]]*}"}"
scratch_root="${scratch_root%"${scratch_root##*[![:space:]]}"}"
if [[ -n "${scratch_root}" ]]; then
    if [[ "${scratch_root}" != /* ]]; then
        echo "error: factory.scratch_root must be an absolute path, got '${scratch_root}'" >&2
        exit 2
    fi
    cache_root="${scratch_root%/}/$(project_key "${cas_dir}")/build-cache"
else
    cache_root="${cas_dir}/build-cache"
fi
snapshots_dir="${cache_root}/snapshots"
source_commit="$(git -C "${repo_root}" rev-parse HEAD)"
commit="${source_commit:0:12}"
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

echo "Worker build-cache root: ${cache_root}"
echo "Building quiescent worker target snapshot: ${snapshot_dir}"
echo "+ CARGO_TARGET_DIR=${snapshot_dir} ${cargo_bin} check ${cargo_args[*]}"
(
    cd "${repo_root}"
    CARGO_TARGET_DIR="${snapshot_dir}" "${cargo_bin}" check "${cargo_args[@]}"
)

# Keep provenance beside the immutable artifacts. Workers use this to reject
# snapshots from unrelated history and to filter artifacts for crates that
# changed after the snapshot was built.
printf 'source_commit=%s\ncreated_at_unix=%s\n' "${source_commit}" "$(date -u +%s)" > "${snapshot_dir}/.cas-build-cache-metadata"

# The snapshot is never written again. Publishing a small pointer file through
# rename keeps worktree provisioning on either the complete old snapshot or
# the complete new snapshot, never a half-built directory.
printf '%s\n' "${snapshot_name}" > "${pointer_next}"
mv -f -- "${pointer_next}" "${cache_root}/current"
trap - EXIT

echo "Published worker target baseline: ${snapshot_name}"
echo "Old snapshots remain valid for in-flight seeders; remove them only during a quiescent maintenance window."
