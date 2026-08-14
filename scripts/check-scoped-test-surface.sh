#!/usr/bin/env bash
#
# Check whether one scoped-test invocation covers the committed source and
# integration-test surfaces it is being offered as proof for. This intentionally
# does not run cargo: run-scoped-tests.sh owns execution and calls this only
# after Cargo has reported a real, nonzero test count.

set -euo pipefail

usage() {
    echo "usage: $0 [--base <git-ref>] -- <cargo/nextest arguments>" >&2
    exit 2
}

base_ref=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --base)
            [[ $# -ge 2 ]] || usage
            base_ref="$2"
            shift 2
            ;;
        --)
            shift
            break
            ;;
        *) usage ;;
    esac
done

[[ $# -gt 0 ]] || usage

repo_root="$(git rev-parse --show-toplevel 2>/dev/null)" || {
    echo "SCOPED PROOF SURFACE: cannot inspect the committed diff outside a Git repository." >&2
    exit 2
}
cd "$repo_root"

if [[ -z "$base_ref" ]]; then
    if git rev-parse --verify --quiet origin/main >/dev/null; then
        base_ref="origin/main"
    elif git rev-parse --verify --quiet main >/dev/null; then
        base_ref="main"
    else
        base_ref="HEAD^"
    fi
fi

merge_base="$(git merge-base "$base_ref" HEAD 2>/dev/null)" || {
    echo "SCOPED PROOF SURFACE: cannot find a merge-base between '$base_ref' and HEAD." >&2
    exit 2
}

lib_requested=false
test_targets=()
filters=()
while [[ $# -gt 0 ]]; do
    case "$1" in
        --lib)
            lib_requested=true
            ;;
        --test)
            [[ $# -ge 2 ]] || usage
            test_targets+=("$2")
            shift
            ;;
        -p|--package|-E|--filter-expr)
            [[ $# -ge 2 ]] || usage
            shift
            ;;
        --*)
            ;;
        *)
            filters+=("$1")
            ;;
    esac
    shift
done

contains_exact() {
    local wanted="$1" value
    shift
    for value in "$@"; do
        [[ "$value" == "$wanted" ]] && return 0
    done
    return 1
}

integration_target_for() {
    local nested_path="$1" directory candidate
    directory="${nested_path%%/*}"
    for candidate in cas-cli/tests/*.rs; do
        [[ -f "$candidate" ]] || continue
        if grep -qF "${directory}/" "$candidate"; then
            basename "${candidate%.rs}"
            return 0
        fi
    done
    # An unfamiliar nested layout is still named loudly rather than skipped.
    printf '%s\n' "$directory"
}

lib_filter_covers() {
    local module="$1" filter
    # An unfiltered --lib run covers every library module. A module filter must
    # name the module itself, not one newly-added test inside it.
    [[ ${#filters[@]} -eq 0 ]] && return 0
    for filter in "${filters[@]}"; do
        [[ "$filter" == "$module" || "$filter" == *"::$module" ]] && return 0
    done
    return 1
}

required_lib_modules=()
required_test_targets=()
while IFS= read -r path; do
    case "$path" in
        cas-cli/src/*.rs)
            source_file="$repo_root/$path"
            [[ -f "$source_file" ]] || continue
            found_test_module=false
            while IFS= read -r module; do
                [[ -z "$module" ]] && continue
                required_lib_modules+=("$module")
                found_test_module=true
            done < <(sed -nE 's/^[[:space:]]*mod[[:space:]]+([[:alnum:]_]*tests)[[:space:]]*\{.*/\1/p' "$source_file")
            if ! "$found_test_module"; then
                required_lib_modules+=("$(basename "${path%.rs}")")
            fi
            ;;
        cas-cli/tests/*.rs)
            required_test_targets+=("$(basename "${path%.rs}")")
            ;;
        cas-cli/tests/*/*.rs)
            nested="${path#cas-cli/tests/}"
            required_test_targets+=("$(integration_target_for "$nested")")
            ;;
    esac
done < <(git diff --name-only "$merge_base" HEAD)

missing=()
for module in "${required_lib_modules[@]}"; do
    if ! "$lib_requested" || ! lib_filter_covers "$module"; then
        missing+=("library module '$module'")
    fi
done
for target in "${required_test_targets[@]}"; do
    if ! contains_exact "$target" "${test_targets[@]}"; then
        missing+=("integration target '$target'")
    fi
done

if [[ ${#missing[@]} -eq 0 ]]; then
    echo "SCOPED PROOF SURFACE: covered committed diff from ${base_ref} ($(git rev-parse --short "$merge_base"))."
    exit 0
fi

echo "SCOPED PROOF SURFACE INCOMPLETE: this invocation does not cover the committed diff from ${base_ref} ($(git rev-parse --short "$merge_base"))." >&2
for item in "${missing[@]}"; do
    echo "  - missing ${item}" >&2
done
echo "Use --proof only for a final receipt that covers every listed surface; ordinary narrow iteration is allowed without it." >&2
exit 1
