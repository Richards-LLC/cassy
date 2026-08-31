#!/usr/bin/env bash
# Run integration snapshot tests when their committed CLI-output inputs change.
#
# Scoped Validation already runs the library target. This router covers
# integration snapshots whose output is produced by a CLI command and therefore
# is invisible to that library-only invocation. Keep the inventory explicit:
# an unrecognized snapshot file must fail loudly instead of silently losing
# pre-merge coverage.

set -euo pipefail

usage() {
    echo "usage: $0 [--base-sha <git-ref>] [--zero-base-ref <git-ref>]" >&2
    exit 2
}

base_ref=""
zero_base_ref=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base-sha)
            [[ $# -ge 2 ]] || usage
            base_ref="$2"
            shift 2
            ;;
        --zero-base-ref)
            [[ $# -ge 2 ]] || usage
            zero_base_ref="$2"
            shift 2
            ;;
        *) usage ;;
    esac
done

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
    echo "snapshot routing: cannot inspect a checkout outside a Git repository" >&2
    exit 2
}
cd "$repo_root"

# snapshot filename | input source surface | integration test target
snapshot_mappings=(
    "component_output_test__doctor_snapshot.snap|cas-cli/src/cli/doctor.rs|component_output_test"
    "component_output_test__status_empty_snapshot.snap|cas-cli/src/cli/status.rs|component_output_test"
)

snapshot_dir="$repo_root/cas-cli/tests/snapshots"
shopt -s nullglob
snapshot_paths=("$snapshot_dir"/*.snap)
if [[ ${#snapshot_paths[@]} -eq 0 ]]; then
    echo "snapshot routing: no committed integration snapshots found in $snapshot_dir" >&2
    exit 1
fi

mapping_for_snapshot() {
    local wanted="$1" entry snapshot surface target
    for entry in "${snapshot_mappings[@]}"; do
        IFS='|' read -r snapshot surface target <<<"$entry"
        if [[ "$snapshot" == "$wanted" ]]; then
            printf '%s\n' "$entry"
            return 0
        fi
    done
    return 1
}

for snapshot_path in "${snapshot_paths[@]}"; do
    snapshot_name="${snapshot_path#"$snapshot_dir/"}"
    if ! mapping_for_snapshot "$snapshot_name" >/dev/null; then
        echo "snapshot routing: no Scoped Validation mapping for $snapshot_name" >&2
        echo "Add its snapshot, input surface, and integration target to snapshot_mappings." >&2
        exit 1
    fi
done

for entry in "${snapshot_mappings[@]}"; do
    IFS='|' read -r snapshot surface target <<<"$entry"
    if [[ ! -f "$snapshot_dir/$snapshot" ]]; then
        echo "snapshot routing: mapping names missing snapshot $snapshot" >&2
        exit 1
    fi
    if [[ ! -f "$repo_root/$surface" ]]; then
        echo "snapshot routing: mapping names missing input surface $surface" >&2
        exit 1
    fi
    if [[ ! -f "$repo_root/cas-cli/tests/$target.rs" ]]; then
        echo "snapshot routing: mapping names missing test target cas-cli/tests/$target.rs" >&2
        exit 1
    fi
done

comparison_base="$base_ref"
if [[ "$comparison_base" =~ ^0+$ && -n "$zero_base_ref" ]]; then
    if git rev-parse --verify --quiet "$zero_base_ref^{commit}" >/dev/null; then
        comparison_base="$zero_base_ref"
        echo "snapshot routing: all-zero event base replaced with $zero_base_ref"
    else
        comparison_base=""
        echo "snapshot routing: trusted fallback $zero_base_ref is unavailable; running all mapped snapshots"
    fi
fi

changed_paths=()
if [[ -n "$comparison_base" && ! "$comparison_base" =~ ^0+$ ]]; then
    if merge_base="$(git merge-base "$comparison_base" HEAD 2>/dev/null)"; then
        while IFS= read -r path; do
            [[ -n "$path" ]] && changed_paths+=("$path")
        done < <(git diff --name-only "$merge_base" HEAD)
    else
        echo "snapshot routing: cannot find merge-base with $comparison_base; running all mapped snapshots"
        comparison_base=""
    fi
else
    echo "snapshot routing: no comparable base; running all mapped snapshots"
fi

targets=()
add_target() {
    local wanted="$1" target
    for target in "${targets[@]}"; do
        [[ "$target" == "$wanted" ]] && return 0
    done
    targets+=("$wanted")
}

if [[ ${#changed_paths[@]} -eq 0 && -n "$comparison_base" ]]; then
    echo "snapshot routing: no mapped CLI-output surface changed; integration snapshot tests not required"
    exit 0
fi

for path in "${changed_paths[@]}"; do
    for entry in "${snapshot_mappings[@]}"; do
        IFS='|' read -r snapshot surface target <<<"$entry"
        if [[ "$path" == "$surface" || "$path" == "cas-cli/tests/snapshots/$snapshot" || "$path" == "cas-cli/tests/$target.rs" ]]; then
            add_target "$target"
        fi
    done
done

if [[ ${#changed_paths[@]} -eq 0 ]]; then
    for entry in "${snapshot_mappings[@]}"; do
        IFS='|' read -r _ surface target <<<"$entry"
        add_target "$target"
    done
fi

if [[ ${#targets[@]} -eq 0 ]]; then
    echo "snapshot routing: mapped snapshot inventory is unchanged; integration snapshot tests not required"
    exit 0
fi

for target in "${targets[@]}"; do
    echo "snapshot routing: changed input surface requires -p cas --test $target"
    "$repo_root/scripts/run-scoped-tests.sh" -p cas --test "$target" --no-fail-fast
done
