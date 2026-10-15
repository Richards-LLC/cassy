#!/usr/bin/env bash
# Decide whether the checked-out tree is a *pending release* tree: the exact
# contents the release workflow is about to tag `vX.Y.Z` and publish.
#
# The version bump rides in the epic PR (scripts/release.sh), so the release
# tree exists the moment that PR enters the merge queue. release-prebuild.yml
# asks this script, on the merge_group tree, whether to start the release
# builds; release.yml asks it again on the main push whether to publish.
#
# A tree is pending exactly when:
#   1. every release crate carries the same version, and
#   2. CHANGELOG.md has a heading for that version, and
#   3. no `v<version>` tag exists yet, and
#   4. with --base-version <v>: the version differs from <v> (the tree the
#      change lands on), so an ordinary PR never opens the release lanes.
#
# (3) makes this self-disarming: once the tag is pushed, later main pushes at
# the same version report pending=false and cost one cheap gate job.
#
# Output lines: pending=, version=, tag=, changed=, reason=. changed=true means
# the version moved relative to --base-version; with pending=false that is a
# broken release (a reused or lowered version) and the caller fails loudly.
set -euo pipefail

base_version=""
while (($#)); do
    case "$1" in
        --base-version)
            base_version="${2:-}"
            shift 2 || shift
            ;;
        --base-version=*)
            base_version="${1#--base-version=}"
            shift
            ;;
        *)
            printf 'usage: %s [--base-version <version>]\n' "$0" >&2
            exit 2
            ;;
    esac
done

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

changed=false

not_pending() {
    printf 'pending=false\n'
    printf 'version=%s\n' "${version:-}"
    printf 'tag=\n'
    printf 'changed=%s\n' "$changed"
    printf 'reason=%s\n' "$1"
    exit 0
}

version="$(crate_version "cas-cli/Cargo.toml")"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    not_pending "cas-cli/Cargo.toml version is not a release semver: ${version:-<empty>}"
fi

if [[ -n "$base_version" ]]; then
    if [[ "$version" == "$base_version" ]]; then
        not_pending "version $version is unchanged from the base tree"
    fi
    changed=true
    highest="$(printf '%s\n%s\n' "$base_version" "$version" | sort -V | tail -n 1)"
    if [[ "$highest" != "$version" ]]; then
        not_pending "version $version is lower than the base tree's $base_version"
    fi
fi

for manifest in "${release_crates[@]}"; do
    actual="$(crate_version "$manifest")"
    if [[ "$actual" != "$version" ]]; then
        not_pending "$manifest is $actual, not $version; the release crates disagree"
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
printf 'changed=%s\n' "$changed"
printf 'reason=%s\n' "tree is at $version with no $tag yet"
