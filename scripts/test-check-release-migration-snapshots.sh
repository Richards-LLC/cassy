#!/usr/bin/env bash
# Self-test for scripts/check-release-migration-snapshots.sh.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$script_dir/check-release-migration-snapshots.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

repo="$tmpdir/repo"
cargo_log="$tmpdir/cargo.log"
cargo_stub="$tmpdir/cargo"

git init -q "$repo"
git -C "$repo" config user.email release-guard@example.test
git -C "$repo" config user.name release-guard-test
mkdir -p "$repo/cas-cli/src/migration/migrations"
printf '// baseline\n' >"$repo/cas-cli/src/migration/migrations/mod.rs"
git -C "$repo" add .
git -C "$repo" commit -qm 'baseline migration registry'
git -C "$repo" tag v0.0.1

cat >"$cargo_stub" <<EOF
#!/usr/bin/env bash
printf '%s\\n' "\$*" >>"$cargo_log"
exit "\${CARGO_EXIT:-0}"
EOF
chmod +x "$cargo_stub"

run_guard() {
  (cd "$repo" && CARGO="$cargo_stub" "$guard")
}

assert_no_cargo() {
  if [[ -s "$cargo_log" ]]; then
    echo "FAIL $1: cargo was invoked unexpectedly" >&2
    cat "$cargo_log" >&2
    exit 1
  fi
}

assert_exact_cargo() {
  local description="$1"
  if [[ "$(cat "$cargo_log")" != 'test -p cas --test component_output_test' ]]; then
    echo "FAIL $description: wrong cargo invocation" >&2
    cat "$cargo_log" >&2
    exit 1
  fi
}

: >"$cargo_log"
unchanged_output="$(run_guard)"
assert_no_cargo 'unchanged migration registry'
grep -qF 'migration snapshots not required' <<<"$unchanged_output"
echo 'ok   unchanged registry skips component-output snapshots'

printf '// migration added\n' >>"$repo/cas-cli/src/migration/migrations/mod.rs"
git -C "$repo" add .
git -C "$repo" commit -qm 'register migration'

: >"$cargo_log"
changed_output="$(run_guard)"
assert_exact_cargo 'changed migration registry'
grep -qF 'changed since v0.0.1' <<<"$changed_output"
echo 'ok   changed registry runs exact component-output snapshot command'

: >"$cargo_log"
set +e
failed_output="$(cd "$repo" && CARGO="$cargo_stub" CARGO_EXIT=42 "$guard" 2>&1)"
failed_status=$?
set -e
if [[ "$failed_status" -ne 42 ]]; then
  echo "FAIL snapshot failure propagates: expected 42, got $failed_status" >&2
  echo "$failed_output" >&2
  exit 1
fi
assert_exact_cargo 'snapshot failure'
echo 'ok   snapshot failure prevents the release path from continuing'

no_tag_repo="$tmpdir/no-tag-repo"
git init -q "$no_tag_repo"
git -C "$no_tag_repo" config user.email release-guard@example.test
git -C "$no_tag_repo" config user.name release-guard-test
mkdir -p "$no_tag_repo/cas-cli/src/migration/migrations"
printf '// first release\n' >"$no_tag_repo/cas-cli/src/migration/migrations/mod.rs"
git -C "$no_tag_repo" add .
git -C "$no_tag_repo" commit -qm 'first migration registry'

: >"$cargo_log"
first_output="$(cd "$no_tag_repo" && CARGO="$cargo_stub" "$guard")"
assert_exact_cargo 'no previous tag'
grep -qF 'no previous tag is reachable' <<<"$first_output"
echo 'ok   first release runs snapshots conservatively'

echo 'PASS: release migration snapshot guard behavior verified.'
