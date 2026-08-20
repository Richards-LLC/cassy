#!/usr/bin/env bash
# Deterministic self-test for scripts/detect-pending-release.sh.
#
# The prebuild gate decides whether every main push pays for two full release
# builds. Both of its failure directions are expensive: a false positive burns
# a hosted macOS runner on an ordinary commit, and a false negative silently
# drops the next release back to the slow tag-time path. Prove both.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
detect="$script_dir/detect-pending-release.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0

expect_field() {
    local output="$1" field="$2" expected="$3" label="$4" actual
    actual="$(grep -m1 "^$field=" <<<"$output" | cut -d= -f2-)"
    if [[ "$actual" == "$expected" ]]; then
        printf 'ok   %s\n' "$label"
        pass=$((pass + 1))
    else
        printf 'FAIL %s (expected %s=%s; got %s)\n' "$label" "$field" "$expected" "$actual"
        fail=$((fail + 1))
    fi
}

make_tree() {
    local root="$1" version="$2" changelog_version="$3"
    rm -rf "$root"
    mkdir -p "$root/cas-cli" "$root/crates"
    for crate in cas-types cas-search cas-store cas-core cas-mcp; do
        mkdir -p "$root/crates/$crate"
        printf '[package]\nname = "%s"\nversion = "%s"\n' "$crate" "$version" \
            >"$root/crates/$crate/Cargo.toml"
    done
    printf '[package]\nname = "cas"\nversion = "%s"\n' "$version" >"$root/cas-cli/Cargo.toml"
    printf '# Changelog\n\n## [%s] - 2026-08-20\n\n- thing\n' "$changelog_version" \
        >"$root/CHANGELOG.md"
}

# A bare repo stands in for `origin`, so the published-tag check is exercised
# through the same `git ls-remote` path CI uses.
remote="$tmp/origin.git"
git init --quiet --bare "$remote"
seed="$tmp/seed"
git init --quiet "$seed"
git -C "$seed" config user.email test@example.com
git -C "$seed" config user.name Test
echo seed >"$seed/README.md"
git -C "$seed" add README.md
git -C "$seed" commit --quiet -m seed
git -C "$seed" tag -a v1.0.0 -m v1.0.0
git -C "$seed" push --quiet "$remote" HEAD:refs/heads/main
git -C "$seed" push --quiet "$remote" v1.0.0

tree="$tmp/tree"

# 1. Release-prep tree: versions aligned, changelog present, tag unpublished.
make_tree "$tree" 2.0.0 2.0.0
out="$(RELEASE_TREE_ROOT="$tree" RELEASE_REMOTE="$remote" "$detect")"
expect_field "$out" pending true 'release-prep tree opens the prebuild lanes'
expect_field "$out" version 2.0.0 'pending tree reports its version'
expect_field "$out" tag v2.0.0 'pending tree reports the tag it will be published as'

# 2. Already-published version: the gate must disarm itself after the tag.
make_tree "$tree" 1.0.0 1.0.0
out="$(RELEASE_TREE_ROOT="$tree" RELEASE_REMOTE="$remote" "$detect")"
expect_field "$out" pending false 'a published version never re-triggers a prebuild'

# 3. Ordinary main push mid-train: crate versions disagree.
make_tree "$tree" 2.0.0 2.0.0
printf '[package]\nname = "cas-core"\nversion = "1.9.0"\n' >"$tree/crates/cas-core/Cargo.toml"
out="$(RELEASE_TREE_ROOT="$tree" RELEASE_REMOTE="$remote" "$detect")"
expect_field "$out" pending false 'a half-bumped release train is not a release tree'

# 4. Version bumped but CHANGELOG not written: not a release tree.
make_tree "$tree" 2.1.0 2.0.0
out="$(RELEASE_TREE_ROOT="$tree" RELEASE_REMOTE="$remote" "$detect")"
expect_field "$out" pending false 'a missing CHANGELOG heading is not a release tree'

# 5. Non-semver / workspace-inherited version must not open the lanes.
make_tree "$tree" 2.2.0 2.2.0
printf '[package]\nname = "cas"\nversion.workspace = true\n' >"$tree/cas-cli/Cargo.toml"
out="$(RELEASE_TREE_ROOT="$tree" RELEASE_REMOTE="$remote" "$detect")"
expect_field "$out" pending false 'an unreadable version is not a release tree'

# Every path must exit 0: this gate reports a decision, it never fails a push.
for version in 2.0.0 1.0.0; do
    make_tree "$tree" "$version" "$version"
    if RELEASE_TREE_ROOT="$tree" RELEASE_REMOTE="$remote" "$detect" >/dev/null; then
        printf 'ok   gate exits 0 for version %s\n' "$version"
        pass=$((pass + 1))
    else
        printf 'FAIL gate exited non-zero for version %s\n' "$version"
        fail=$((fail + 1))
    fi
done

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
