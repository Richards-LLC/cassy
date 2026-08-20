#!/usr/bin/env bash
# Decide whether the checked-out tree is a *pending release* tree: the exact
# contents a `vX.Y.Z` tag is about to be pushed at.
#
# This is the trigger for prebuilding release artifacts at release-PR merge
# instead of at tag time (cas-3b7c0). The release tree is final the moment the
# version-bump PR lands on main, so the expensive platform builds no longer
# have to sit on the tag's critical path.
#
# A tree is pending exactly when:
#   1. every release-train crate carries the same version, and
#   2. CHANGELOG.md has a heading for that version, and
#   3. no `v<version>` tag exists yet.
#
# (3) makes this self-disarming: once the tag is pushed, later main pushes at
# the same version report pending=false and cost one cheap gate job.
set -euo pipefail

# RELEASE_TREE_ROOT exists so the self-test can point this at a fixture tree
# instead of the checkout it lives in. CI never sets it.
repo_root="${RELEASE_TREE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$repo_root"

release_crates=(
    "cas-cli/Cargo.toml"
    "crates/cas-types/Cargo.toml"
    "crates/cas-search/Cargo.toml"
    "crates/cas-store/Cargo.toml"
    "crates/cas-core/Cargo.toml"
    "crates/cas-mcp/Cargo.toml"
)

crate_version() {
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$1" | head -n 1
}

not_pending() {
    printf 'pending=false\n'
    printf 'version=\n'
    printf 'tag=\n'
    printf 'reason=%s\n' "$1"
    exit 0
}

version="$(crate_version "cas-cli/Cargo.toml")"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    not_pending "cas-cli/Cargo.toml version is not a release semver: ${version:-<empty>}"
fi

for manifest in "${release_crates[@]}"; do
    actual="$(crate_version "$manifest")"
    if [[ "$actual" != "$version" ]]; then
        not_pending "$manifest is $actual, not $version; release train is mid-bump"
    fi
done

if ! grep -Eq "^## \[$version\]( - [0-9]{4}-[0-9]{2}-[0-9]{2})?$" CHANGELOG.md; then
    not_pending "CHANGELOG.md has no heading for $version"
fi

tag="v$version"
# Ask the remote, not the local ref store: a CI checkout may not have fetched
# tags, and a stale local tag must never suppress a real prebuild.
remote="${RELEASE_REMOTE:-origin}"
if git ls-remote --tags --exit-code "$remote" "refs/tags/$tag" >/dev/null 2>&1; then
    # The disarming condition is the TAG existing, not the release object being
    # published: once the tag is pushed its own workflow owns the artifacts, so
    # prebuilding for that version can no longer help anything.
    not_pending "$tag already exists on $remote"
fi

printf 'pending=true\n'
printf 'version=%s\n' "$version"
printf 'tag=%s\n' "$tag"
printf 'reason=%s\n' "release train is at $version with no $tag yet"
