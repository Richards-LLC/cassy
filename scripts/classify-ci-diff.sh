#!/usr/bin/env bash
# Print a conservative CI diff class. Callers may skip Rust work only for the
# explicitly Rust-unaffected classes; every other path, including an unknown
# one, is rust-touched. This keeps the classifier fail-closed.
set -euo pipefail

usage() {
    echo "usage: $0 <merge-base> <head>" >&2
    exit 2
}

[[ $# == 2 ]] || usage
base="$1"
head="$2"

# Capture the diff with an explicitly checked exit status before reading it.
# A process-substitution producer (`done < <(git diff ...)`) does not fail the
# loop under `set -e`, so an unknown ref or a broken git used to read as an
# empty diff and exit 0 — a fast-pass for an unknown change set (audit finding
# 7, cas-b505). Failing here is fail-closed: the composite action treats a
# nonzero classifier as rust-touched.
if ! diff_list="$(git diff --name-only "$base" "$head")"; then
    echo "classify-ci-diff: git diff failed for $base..$head; refusing to classify" >&2
    exit 1
fi
files=()
while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    files[${#files[@]}]="$file"
done <<<"$diff_list"
if [[ ${#files[@]} == 0 ]]; then
    echo empty
    exit 0
fi

docs_only=true
for file in "${files[@]}"; do
    case "$file" in
        # `cas-cli/src` contains Rust compilation inputs beyond `.rs`: builtin
        # guides, integration templates, and parser fixtures are embedded with
        # `include_str!`/`include_bytes!`. Keep this directory fail-closed
        # regardless of the file extension.
        cas-cli/src/*) docs_only=false; break ;;
        docs/*|*.md) ;;
        *) docs_only=false; break ;;
    esac
done
if "$docs_only"; then
    echo docs-only
    exit 0
fi

# Commander assets are built and tested by their own preflight step. A change
# entirely beneath hub-web/ cannot affect a Rust build, doctest, or macOS
# check, but it must not be confused with a documentation-only change.
hub_web_only=true
for file in "${files[@]}"; do
    case "$file" in
        hub-web/*) ;;
        *) hub_web_only=false; break ;;
    esac
done
if "$hub_web_only"; then
    echo hub-web-only
    exit 0
fi

# A release-only patch changes one or more workspace package manifests and the
# matching package entries in Cargo.lock. Every manifest must change exactly
# one version line; Cargo.lock may change only version lines for those same
# packages. Keep this conservative contract pinned in test-ci-test-tiers.sh.
package_names=()
manifest_versions=()
lock_old_versions=()
lock_new_versions=()
lock_changes=()
has_lock=false
valid_version_bump=true

# Bash 3.2 has indexed arrays but not associative arrays. Keep package metadata
# in parallel indexed arrays and use this lookup helper to retain the same
# uniqueness and matching checks as the associative-array implementation.
package_index() {
    local wanted="$1"
    local index

    for index in "${!package_names[@]}"; do
        if [[ "${package_names[$index]}" == "$wanted" ]]; then
            package_index_result="$index"
            return 0
        fi
    done

    package_index_result=""
    return 1
}

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

    # An assignment from a failing command substitution exits under `set -e`;
    # the version-bump and lock diffs below therefore already fail closed.
    manifest_diff="$(git diff --unified=0 "$base" "$head" -- "$file")"
    manifest_change_count=0
    manifest_old_change=""
    manifest_new_change=""
    while IFS= read -r manifest_change; do
        manifest_change_count=$((manifest_change_count + 1))
        if [[ "$manifest_change_count" == 1 ]]; then
            manifest_old_change="$manifest_change"
        elif [[ "$manifest_change_count" == 2 ]]; then
            manifest_new_change="$manifest_change"
        fi
    done < <(grep -E '^[+-]' <<<"$manifest_diff" | grep -vE '^(---|\+\+\+)' || true)
    if [[ "$manifest_change_count" != 2 ]] \
        || [[ ! "$manifest_old_change" =~ ^-version\ =\ \"([^\"]+)\"$ ]] \
        || [[ ! "$manifest_new_change" =~ ^\+version\ =\ \"([^\"]+)\"$ ]]; then
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
    if [[ -z "$package_name" ]] || package_index "$package_name"; then
        valid_version_bump=false
        break
    fi
    package_index_result=${#package_names[@]}
    package_names[$package_index_result]="$package_name"
    manifest_versions[$package_index_result]="$manifest_new_version"
    lock_old_versions[$package_index_result]=""
    lock_new_versions[$package_index_result]=""
    lock_changes[$package_index_result]=0
done

if "$valid_version_bump" && "$has_lock" && [[ ${#package_names[@]} -gt 0 ]]; then
    lock_diff="$(git diff --unified=0 "$base" "$head" -- Cargo.lock)"
    lock_package=""
    while IFS= read -r line; do
        if [[ "$line" == ---* || "$line" == +++* ]]; then
            continue
        elif [[ "$line" =~ ^@@.*name\ =\ \"([^\"]+)\"$ ]]; then
            lock_package="${BASH_REMATCH[1]}"
        elif [[ "$line" == -* || "$line" == +* ]]; then
            if [[ -z "$lock_package" ]] || ! package_index "$lock_package" \
                || [[ ! "$line" =~ ^[-+]version\ =\ \"([^\"]+)\"$ ]]; then
                valid_version_bump=false
                break
            fi
            package_slot="$package_index_result"
            lock_changes[$package_slot]=$(( ${lock_changes[$package_slot]} + 1 ))
            if [[ "$line" == -* ]]; then
                lock_old_versions[$package_slot]="${BASH_REMATCH[1]}"
            else
                lock_new_versions[$package_slot]="${BASH_REMATCH[1]}"
            fi
        fi
    done <<<"$lock_diff"

    for package_slot in "${!package_names[@]}"; do
        if [[ "${lock_changes[$package_slot]}" != 2 ]] \
            || [[ -z "${lock_old_versions[$package_slot]}" ]] \
            || [[ "${lock_new_versions[$package_slot]}" != "${manifest_versions[$package_slot]}" ]]; then
            valid_version_bump=false
            break
        fi
    done
fi

if "$valid_version_bump" && "$has_lock" && [[ ${#package_names[@]} -gt 0 ]]; then
    echo version-bump
    exit 0
fi

echo rust-touched
