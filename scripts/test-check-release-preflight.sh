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

echo 'PASS: release preflight guard behavior verified.'
