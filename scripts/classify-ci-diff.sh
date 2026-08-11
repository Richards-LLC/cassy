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

# A release-only patch is the exact two-line bump Cargo currently generates:
# the cas manifest's package version and the matching `cas` lock entry. Do not
# broaden this without changing the pinned fixtures in test-ci-test-tiers.sh.
if [[ "${files[*]}" == "Cargo.lock cas-cli/Cargo.toml" || "${files[*]}" == "cas-cli/Cargo.toml Cargo.lock" ]]; then
    manifest_diff="$(git diff --unified=0 "$base" "$head" -- cas-cli/Cargo.toml)"
    lock_diff="$(git diff --unified=0 "$base" "$head" -- Cargo.lock)"
    if grep -qE '^-version = "[0-9]+\.[0-9]+\.[0-9]+"$' <<<"$manifest_diff" \
        && grep -qE '^\+version = "[0-9]+\.[0-9]+\.[0-9]+"$' <<<"$manifest_diff" \
        && grep -qE '^-version = "[0-9]+\.[0-9]+\.[0-9]+"$' <<<"$lock_diff" \
        && grep -qE '^\+version = "[0-9]+\.[0-9]+\.[0-9]+"$' <<<"$lock_diff" \
        && [[ "$(grep -Ec '^[+-](version = )' <<<"$manifest_diff")" == 2 ]] \
        && [[ "$(grep -Ec '^[+-](version = )' <<<"$lock_diff")" == 2 ]] \
        && grep -q '^name = "cas"$' < <(git show "$head:Cargo.lock") \
        && grep -qE '^@@ .* name = "cas"$' <<<"$lock_diff"; then
        echo version-bump
        exit 0
    fi
fi

echo full
