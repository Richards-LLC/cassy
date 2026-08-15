#!/usr/bin/env bash
# Self-test for scripts/check-release-preflight.sh without compiling Rust.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-release-preflight.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

repo="$tmpdir/repo"
remote="$tmpdir/origin.git"
cargo_log="$tmpdir/cargo.log"
cargo_stub="$tmpdir/cargo"

git init --bare -q "$remote"
git init -q "$repo"
git -C "$repo" config user.email release-guard@example.test
git -C "$repo" config user.name release-guard-test
git -C "$repo" remote add origin "$remote"
mkdir -p "$repo/cas-cli" "$repo/crates"/{cas-types,cas-search,cas-store,cas-core,cas-mcp}
for manifest in cas-cli/Cargo.toml crates/{cas-types,cas-search,cas-store,cas-core,cas-mcp}/Cargo.toml; do
  printf '[package]\nversion = "1.2.3"\n' >"$repo/$manifest"
done
printf '## [1.2.3] - 2026-08-09\n\n- Release fixture.\n' >"$repo/CHANGELOG.md"
git -C "$repo" add .
git -C "$repo" commit -qm 'release fixture'
git -C "$repo" tag -a v1.2.3 -m v1.2.3
git -C "$repo" push -q origin HEAD:main
git -C "$repo" push -q origin v1.2.3

cat >"$cargo_stub" <<EOF
#!/usr/bin/env bash
printf '%s\\n' "\$*" >>"$cargo_log"
exit "\${CARGO_EXIT:-0}"
EOF
chmod +x "$cargo_stub"

run_guard() {
  (cd "$repo" && CARGO="$cargo_stub" "$guard" v1.2.3)
}

run_guard_local() {
  (cd "$repo" && CARGO="$cargo_stub" "$guard" --local v1.2.3)
}

: >"$cargo_log"
run_guard >/dev/null
test "$(cat "$cargo_log")" = 'check --locked'
echo 'ok   clean annotated release tag passes all preflight checks'

# actions/checkout may leave only the peeled tag target in the local ref. The
# guard must refresh the authoritative remote tag object and still pass.
git -C "$repo" update-ref refs/tags/v1.2.3 "$(git -C "$repo" rev-parse v1.2.3^{})"
: >"$cargo_log"
run_guard >/dev/null
test "$(git -C "$repo" cat-file -t refs/tags/v1.2.3)" = 'tag'
test "$(cat "$cargo_log")" = 'check --locked'
echo 'ok   peeled checkout tag ref is refreshed to the annotated remote tag'

printf 'dirty\n' >"$repo/uncommitted.txt"
: >"$cargo_log"
set +e
dirty_output="$(run_guard 2>&1)"
dirty_status=$?
set -e
test "$dirty_status" -ne 0
grep -qF 'release input is dirty' <<<"$dirty_output"
test ! -s "$cargo_log"
rm "$repo/uncommitted.txt"
echo 'ok   dirty release fails before cargo check'

sed -i.bak 's/1.2.3/9.9.9/' "$repo/crates/cas-core/Cargo.toml"
rm "$repo/crates/cas-core/Cargo.toml.bak"
git -C "$repo" add crates/cas-core/Cargo.toml
git -C "$repo" commit -qm 'introduce release-train version drift'
git -C "$repo" tag -fa v1.2.3 -m v1.2.3 >/dev/null
git -C "$repo" push -q --force origin v1.2.3
: >"$cargo_log"
set +e
version_output="$(run_guard 2>&1)"
version_status=$?
set -e
test "$version_status" -ne 0
grep -qF 'cas-core/Cargo.toml is 9.9.9; expected 1.2.3' <<<"$version_output"
test ! -s "$cargo_log"
echo 'ok   release-train version drift fails before cargo check'

# Refreshing the tag object must not make a lightweight remote tag acceptable.
git -C "$repo" tag -d v1.2.3 >/dev/null
git -C "$repo" tag v1.2.3
git -C "$repo" push -q --force origin v1.2.3
: >"$cargo_log"
set +e
lightweight_output="$(run_guard 2>&1)"
lightweight_status=$?
set -e
test "$lightweight_status" -ne 0
grep -qF 'v1.2.3 must exist locally as an annotated tag' <<<"$lightweight_output"
test ! -s "$cargo_log"
echo 'ok   lightweight remote tag fails annotated-tag preflight'

# --- pre-push (local) lane -------------------------------------------------
# release.sh runs the guard before the tag is pushed, so a second fixture is
# built whose annotated tag exists only locally.
new_fixture() {
  local name="$1"
  local fixture_repo="$tmpdir/$name"
  local fixture_remote="$tmpdir/$name-origin.git"
  git init --bare -q "$fixture_remote"
  git init -q "$fixture_repo"
  git -C "$fixture_repo" config user.email release-guard@example.test
  git -C "$fixture_repo" config user.name release-guard-test
  git -C "$fixture_repo" remote add origin "$fixture_remote"
  mkdir -p "$fixture_repo/cas-cli" "$fixture_repo/crates"/{cas-types,cas-search,cas-store,cas-core,cas-mcp}
  for m in cas-cli/Cargo.toml crates/{cas-types,cas-search,cas-store,cas-core,cas-mcp}/Cargo.toml; do
    printf '[package]\nversion = "1.2.3"\n' >"$fixture_repo/$m"
  done
  printf '## [1.2.3] - 2026-08-09\n\n- Release fixture.\n' >"$fixture_repo/CHANGELOG.md"
  git -C "$fixture_repo" add .
  git -C "$fixture_repo" commit -qm 'release fixture'
  git -C "$fixture_repo" push -q origin HEAD:main
  printf '%s' "$fixture_repo"
}

prepush="$(new_fixture prepush)"
git -C "$prepush" tag -a v1.2.3 -m v1.2.3
run_prepush() {
  (cd "$prepush" && CARGO="$cargo_stub" "$guard" "$@")
}

: >"$cargo_log"
run_prepush --local v1.2.3 >/dev/null
test "$(cat "$cargo_log")" = 'check --locked'
test -z "$(git -C "$prepush" ls-remote --tags origin 2>/dev/null)"
echo 'ok   local lane passes with an annotated tag that is not on the remote yet'

# The CI lane must keep re-fetching the remote tag object; without the push it
# cannot pass, which is exactly why release.sh needs the --local lane.
: >"$cargo_log"
set +e
ci_no_remote_output="$(run_prepush v1.2.3 2>&1)"
ci_no_remote_status=$?
set -e
test "$ci_no_remote_status" -ne 0
grep -qF "couldn't find remote ref" <<<"$ci_no_remote_output"
test ! -s "$cargo_log"
echo 'ok   CI lane still requires the tag to exist on the remote'

# Local gates keep their teeth in --local mode.
printf 'dirty\n' >"$prepush/uncommitted.txt"
: >"$cargo_log"
set +e
local_dirty_output="$(run_prepush --local v1.2.3 2>&1)"
local_dirty_status=$?
set -e
test "$local_dirty_status" -ne 0
grep -qF 'release input is dirty' <<<"$local_dirty_output"
test ! -s "$cargo_log"
rm "$prepush/uncommitted.txt"
echo 'ok   local lane rejects a dirty tree'

git -C "$prepush" commit -q --allow-empty -m 'commit after tagging'
: >"$cargo_log"
set +e
local_stale_output="$(run_prepush --local v1.2.3 2>&1)"
local_stale_status=$?
set -e
test "$local_stale_status" -ne 0
grep -qF 'but this build checks out' <<<"$local_stale_output"
test ! -s "$cargo_log"
echo 'ok   local lane rejects a tag that no longer peels to HEAD'

git -C "$prepush" tag -d v1.2.3 >/dev/null
git -C "$prepush" tag v1.2.3
: >"$cargo_log"
set +e
local_lightweight_output="$(run_prepush --local v1.2.3 2>&1)"
local_lightweight_status=$?
set -e
test "$local_lightweight_status" -ne 0
grep -qF 'v1.2.3 must exist locally as an annotated tag' <<<"$local_lightweight_output"
test ! -s "$cargo_log"
echo 'ok   local lane rejects a lightweight tag'

# --- CI lane rejects a re-pointed remote tag -------------------------------
repointed="$(new_fixture repointed)"
git -C "$repointed" tag -a v1.2.3 -m v1.2.3
git -C "$repointed" push -q origin v1.2.3
git -C "$repointed" commit -q --allow-empty -m 'commit published after the tag'
git -C "$repointed" push -q origin HEAD:main
: >"$cargo_log"
set +e
repointed_output="$( (cd "$repointed" && CARGO="$cargo_stub" "$guard" v1.2.3) 2>&1 )"
repointed_status=$?
set -e
test "$repointed_status" -ne 0
grep -qF 'but this build checks out' <<<"$repointed_output"
test ! -s "$cargo_log"
echo 'ok   CI lane rejects a remote tag that does not peel to the build commit'

# Unknown flags are rejected rather than silently treated as the tag argument.
set +e
badflag_output="$(run_prepush --nope v1.2.3 2>&1)"
badflag_status=$?
set -e
test "$badflag_status" -eq 2
grep -qF 'unknown option --nope' <<<"$badflag_output"
echo 'ok   unknown option is rejected'

echo 'PASS: release preflight guard behavior verified.'
