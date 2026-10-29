#!/usr/bin/env bash
# Deterministic self-test for scripts/release-publish.sh (cas-f95b).
#
# The publisher is the only step holding a contents:write token. Prove it
# refuses to replace a published release, refuses a tag at another commit,
# requires both platform assets, pushes an annotated tag at the landed commit,
# and writes the CHANGELOG section plus the PR-opened timer into the notes.
set -euo pipefail
# A git hook (the pre-push fast rows) exports GIT_DIR and friends. A fixture
# `git init` would then reinitialise the real repository (cas-f95b), so drop
# every repository-local git variable before touching a fixture repository.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars)

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
publish="$script_dir/release-publish.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }

mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${FAKE_GH_LOG:?}"
case "$1 $2" in
  "release view") exit "${FAKE_RELEASE_EXISTS:-1}" ;;
  "release create") exit 0 ;;
  "api repos/"*) printf '%s\n' "${FAKE_PR_JSON:-}" ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"

remote="$tmp/origin.git"
git init --quiet --bare "$remote"
repo="$tmp/repo"
git init --quiet "$repo"
git -C "$repo" config user.email test@example.com
git -C "$repo" config user.name Test
git -C "$repo" remote add origin "$remote"
cat >"$repo/CHANGELOG.md" <<'EOF'
# Changelog

## [Unreleased]

## [9.99.2] - 2026-10-15

### Added — the fixture entry

- One shipped change.

## [9.99.1] - 2026-10-14

- The previous release.
EOF
git -C "$repo" add CHANGELOG.md
git -C "$repo" commit --quiet -m 'release: v9.99.2'
git -C "$repo" push --quiet origin HEAD:refs/heads/main
sha="$(git -C "$repo" rev-parse HEAD)"
assets="$tmp/assets"
mkdir -p "$assets"
echo linux >"$assets/cas-x86_64-unknown-linux-gnu.tar.gz"
echo macos >"$assets/cas-aarch64-apple-darwin.tar.gz"

run_publish() {
    (cd "$repo" && GH_BIN="$tmp/bin/gh" GITHUB_REPOSITORY=example/repo FAKE_GH_LOG="$tmp/gh.log" \
        RELEASE_PUBLISH_NOW=1760500000 "$publish" "$@")
}

# 1. An existing release is never replaced.
: >"$tmp/gh.log"
set +e
output="$(FAKE_RELEASE_EXISTS=0 run_publish --version 9.99.2 --sha "$sha" --assets "$assets" 2>&1)"
status=$?
set -e
check '[[ $status -eq 1 && "$output" == *"already exists; refusing to replace its assets"* ]]' 'an existing release refuses loudly'
check '! grep -q "release create" "$tmp/gh.log"' 'an existing release uploads no replacement bytes'
check '[[ -z "$(git -C "$remote" tag)" ]]' 'an existing release pushes no tag'

# 2. A missing platform asset is refused before anything is published.
mv "$assets/cas-aarch64-apple-darwin.tar.gz" "$tmp/held.tar.gz"
set +e
output="$(run_publish --version 9.99.2 --sha "$sha" --assets "$assets" 2>&1)"
status=$?
set -e
mv "$tmp/held.tar.gz" "$assets/cas-aarch64-apple-darwin.tar.gz"
check '[[ $status -ne 0 && "$output" == *"missing release asset cas-aarch64-apple-darwin.tar.gz"* ]]' 'a missing macOS asset is refused'

# 3. First publication: annotated tag at the commit, notes, release with assets.
: >"$tmp/gh.log"
created=$((1760500000 - 25 * 60))
pr_json="{\"number\":4242,\"created_at\":\"$(date -u -d "@$created" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -r "$created" +%Y-%m-%dT%H:%M:%SZ)\"}"
FAKE_PR_JSON="$pr_json" run_publish --version 9.99.2 --sha "$sha" --assets "$assets" \
    --notes-out "$tmp/notes.md" >/dev/null
check '[[ "$(git -C "$remote" cat-file -t refs/tags/v9.99.2)" == tag ]]' 'the pushed tag is annotated'
check '[[ "$(git -C "$remote" rev-parse "refs/tags/v9.99.2^{}")" == "$sha" ]]' 'the tag peels to the landed commit'
check 'grep -q "release create v9.99.2 --repo example/repo --verify-tag --title CAS v9.99.2" "$tmp/gh.log"' 'the release is created for the verified tag'
check 'grep -q "cas-x86_64-unknown-linux-gnu.tar.gz" "$tmp/gh.log" && grep -q "cas-aarch64-apple-darwin.tar.gz" "$tmp/gh.log"' 'both platform assets are uploaded'
check '! grep -q -- "--prerelease" "$tmp/gh.log"' 'a real release is not a prerelease'
check 'grep -q "^- One shipped change.$" "$tmp/notes.md" && ! grep -q "The previous release" "$tmp/notes.md"' 'notes carry exactly the version CHANGELOG section'
check 'grep -q "PR #4242 opened .* published .*: 25 min. Installed: pending." "$tmp/notes.md"' 'notes print the PR-opened timer'
check 'grep -q "<!-- cas-release-timer pr=4242 opened=$created published=1760500000 -->" "$tmp/notes.md"' 'notes carry the timer marker the host install completes'

# 4. A retry after the tag push accepts the tag at the same commit.
: >"$tmp/gh.log"
run_publish --version 9.99.2 --sha "$sha" --assets "$assets" >/dev/null
check 'grep -q "release create v9.99.2" "$tmp/gh.log"' 'a retry reuses a tag that already peels to the commit'

# 5. A tag at another commit is refused.
echo more >>"$repo/CHANGELOG.md"
git -C "$repo" commit --quiet -am other
other="$(git -C "$repo" rev-parse HEAD)"
: >"$tmp/gh.log"
set +e
output="$(run_publish --version 9.99.2 --sha "$other" --assets "$assets" 2>&1)"
status=$?
set -e
check '[[ $status -eq 1 && "$output" == *"already exists at $sha, not $other"* ]]' 'a tag at another commit is refused'
check '! grep -q "release create" "$tmp/gh.log"' 'a refused tag publishes nothing'

# 6. A dry run publishes a prerelease under its own tag.
: >"$tmp/gh.log"
run_publish --version 9.99.2 --sha "$other" --assets "$assets" --tag dryrun-v9.99.2-1 --prerelease >/dev/null
check 'grep -q "release create dryrun-v9.99.2-1 .*--prerelease" "$tmp/gh.log"' 'a dry run creates a prerelease'
check '[[ "$(git -C "$remote" rev-parse "refs/tags/dryrun-v9.99.2-1^{}")" == "$other" ]]' 'a dry-run tag peels to the dispatched commit'
check 'grep -q "no merged PR found" <(run_publish --version 9.99.2 --sha "$other" --assets "$assets" --tag dryrun-v9.99.2-2 --notes-out /dev/stdout 2>/dev/null)' 'a commit with no merged PR still publishes with a timer line'

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
