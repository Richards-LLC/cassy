#!/usr/bin/env bash
# Print the only diff classes permitted to avoid the required CI workloads.
# This is deliberately fail-closed: callers should run their normal job for
# any output other than empty, docs-only, or version-bump.
set -euo pipefail

usage() {
    echo "usage: $0 <merge-base> <head>" >&2
    exit 2
}

[[ $# == 2 ]] || usage
base="$1"
head="$2"

mapfile -t files < <(git diff --name-only "$base" "$head")
if [[ ${#files[@]} == 0 ]]; then
    echo empty
    exit 0
fi

docs_only=true
for file in "${files[@]}"; do
    case "$file" in
        docs/*|*.md) ;;
        *) docs_only=false; break ;;
    esac
done
if "$docs_only"; then
    echo docs-only
    exit 0
fi

# A release-only patch changes one or more workspace package manifests and the
# matching package entries in Cargo.lock. Every manifest must change exactly
# one version line; Cargo.lock may change only version lines for those same
# packages. Keep this conservative contract pinned in test-ci-test-tiers.sh.
declare -A manifest_versions lock_old_versions lock_new_versions lock_changes
has_lock=false
valid_version_bump=true

for file in "${files[@]}"; do
    if [[ "$file" == Cargo.lock ]]; then
        has_lock=true
        continue
    fi

    manifest_path="${file%/Cargo.toml}"
    if [[ "$file" != */Cargo.toml ]] \
        || ! git show "$head:Cargo.toml" | grep -qF "    \"$manifest_path\","; then
        valid_version_bump=false
        break
    fi

    manifest_diff="$(git diff --unified=0 "$base" "$head" -- "$file")"
    mapfile -t manifest_changes < <(grep -E '^[+-]' <<<"$manifest_diff" | grep -vE '^(---|\+\+\+)' || true)
    if [[ ${#manifest_changes[@]} != 2 ]] \
        || [[ ! "${manifest_changes[0]}" =~ ^-version\ =\ \"([^\"]+)\"$ ]] \
        || [[ ! "${manifest_changes[1]}" =~ ^\+version\ =\ \"([^\"]+)\"$ ]]; then
        valid_version_bump=false
        break
    fi
    manifest_new_version="${BASH_REMATCH[1]}"

    package_name="$(git show "$head:$file" | awk '
        /^\[package\]$/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && /^name = "/ {
            sub(/^name = "/, "")
            sub(/"$/, "")
            print
            exit
        }
    ')"
    if [[ -z "$package_name" || -n "${manifest_versions[$package_name]+x}" ]]; then
        valid_version_bump=false
        break
    fi
    manifest_versions["$package_name"]="$manifest_new_version"
done

if "$valid_version_bump" && "$has_lock" && [[ ${#manifest_versions[@]} -gt 0 ]]; then
    lock_diff="$(git diff --unified=0 "$base" "$head" -- Cargo.lock)"
    lock_package=""
    while IFS= read -r line; do
        if [[ "$line" == ---* || "$line" == +++* ]]; then
            continue
        elif [[ "$line" =~ ^@@.*name\ =\ \"([^\"]+)\"$ ]]; then
            lock_package="${BASH_REMATCH[1]}"
        elif [[ "$line" == -* || "$line" == +* ]]; then
            if [[ -z "$lock_package" || -z "${manifest_versions[$lock_package]+x}" ]] \
                || [[ ! "$line" =~ ^[-+]version\ =\ \"([^\"]+)\"$ ]]; then
                valid_version_bump=false
                break
            fi
            lock_changes["$lock_package"]=$(( ${lock_changes[$lock_package]:-0} + 1 ))
            if [[ "$line" == -* ]]; then
                lock_old_versions["$lock_package"]="${BASH_REMATCH[1]}"
            else
                lock_new_versions["$lock_package"]="${BASH_REMATCH[1]}"
            fi
        fi
    done <<<"$lock_diff"

    for package_name in "${!manifest_versions[@]}"; do
        if [[ "${lock_changes[$package_name]:-0}" != 2 ]] \
            || [[ -z "${lock_old_versions[$package_name]+x}" ]] \
            || [[ "${lock_new_versions[$package_name]:-}" != "${manifest_versions[$package_name]}" ]]; then
            valid_version_bump=false
            break
        fi
    done
fi

if "$valid_version_bump" && "$has_lock" && [[ ${#manifest_versions[@]} -gt 0 ]]; then
    echo version-bump
    exit 0
fi

echo full
