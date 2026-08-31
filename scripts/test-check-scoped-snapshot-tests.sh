#!/usr/bin/env bash
# Self-test for check-scoped-snapshot-tests.sh.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
router="$script_dir/check-scoped-snapshot-tests.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

repo="$tmpdir/repo"
mkdir -p "$repo/cas-cli/src/cli" "$repo/cas-cli/tests/snapshots" "$repo/scripts"
cp "$router" "$repo/scripts/check-scoped-snapshot-tests.sh"

cat >"$repo/scripts/run-scoped-tests.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${SNAPSHOT_ROUTER_LOG:?}"
if [[ "${SNAPSHOT_ROUTER_STATUS:-0}" != 0 ]]; then
    exit "$SNAPSHOT_ROUTER_STATUS"
fi
printf '%s\n' 'Summary [   0.001s] 2 tests run: 2 passed, 0 skipped'
EOF
chmod +x "$repo/scripts"/*.sh

printf '%s\n' '// doctor implementation' >"$repo/cas-cli/src/cli/doctor.rs"
printf '%s\n' '// status implementation' >"$repo/cas-cli/src/cli/status.rs"
printf '%s\n' '// component output integration test' >"$repo/cas-cli/tests/component_output_test.rs"
printf '%s\n' 'doctor snapshot' >"$repo/cas-cli/tests/snapshots/component_output_test__doctor_snapshot.snap"
printf '%s\n' 'status snapshot' >"$repo/cas-cli/tests/snapshots/component_output_test__status_empty_snapshot.snap"

git -C "$repo" init -q -b main
git -C "$repo" config user.email snapshot-routing@example.test
git -C "$repo" config user.name snapshot-routing-test
git -C "$repo" add .
git -C "$repo" commit -qm baseline
git -C "$repo" checkout -qb doctor-surface

printf '%s\n' '// changed doctor output line' >>"$repo/cas-cli/src/cli/doctor.rs"
git -C "$repo" add .
git -C "$repo" commit -qm 'change doctor output'

: >"$tmpdir/router.log"
doctor_output="$(
    cd "$repo"
    SNAPSHOT_ROUTER_LOG="$tmpdir/router.log" ./scripts/check-scoped-snapshot-tests.sh --base-sha main
)"
grep -qF 'changed input surface requires -p cas --test component_output_test' <<<"$doctor_output"
[[ "$(cat "$tmpdir/router.log")" == '-p cas --test component_output_test --no-fail-fast' ]]
echo 'ok   doctor.rs change routes component_output_test'

: >"$tmpdir/router.log"
set +e
failure_output="$(
    cd "$repo"
    SNAPSHOT_ROUTER_LOG="$tmpdir/router.log" SNAPSHOT_ROUTER_STATUS=101 \
        ./scripts/check-scoped-snapshot-tests.sh --base-sha main 2>&1
)"
failure_status=$?
set -e
[[ "$failure_status" -eq 101 ]]
grep -qF 'changed input surface requires -p cas --test component_output_test' <<<"$failure_output"
echo 'ok   component_output_test failure propagates (simulates run 33435790948)'

git -C "$repo" checkout -q main
git -C "$repo" checkout -qb status-surface
printf '%s\n' '// changed status output line' >>"$repo/cas-cli/src/cli/status.rs"
git -C "$repo" add .
git -C "$repo" commit -qm 'change status output'
: >"$tmpdir/router.log"
status_output="$(
    cd "$repo"
    SNAPSHOT_ROUTER_LOG="$tmpdir/router.log" ./scripts/check-scoped-snapshot-tests.sh --base-sha main
)"
grep -qF 'changed input surface requires -p cas --test component_output_test' <<<"$status_output"
[[ "$(cat "$tmpdir/router.log")" == '-p cas --test component_output_test --no-fail-fast' ]]
echo 'ok   status.rs change routes component_output_test'

git -C "$repo" checkout -q main
printf '%s\n' 'documentation only' >"$repo/README.md"
git -C "$repo" add README.md
git -C "$repo" commit -qm 'change documentation'
: >"$tmpdir/router.log"
docs_output="$(
    cd "$repo"
    SNAPSHOT_ROUTER_LOG="$tmpdir/router.log" ./scripts/check-scoped-snapshot-tests.sh --base-sha main
)"
grep -qF 'no mapped CLI-output surface changed' <<<"$docs_output"
[[ ! -s "$tmpdir/router.log" ]]
echo 'ok   unrelated change skips integration snapshots'

printf '%s\n' 'new snapshot without a mapping' >"$repo/cas-cli/tests/snapshots/component_output_test__new_snapshot.snap"
git -C "$repo" add .
git -C "$repo" commit -qm 'add unmapped snapshot'
set +e
unmapped_output="$(
    cd "$repo"
    ./scripts/check-scoped-snapshot-tests.sh --base-sha main 2>&1
)"
unmapped_status=$?
set -e
[[ "$unmapped_status" -eq 1 ]]
grep -qF 'no Scoped Validation mapping for component_output_test__new_snapshot.snap' <<<"$unmapped_output"
echo 'ok   unmapped snapshot fails the routing guard'

echo 'PASS: scoped snapshot routing verified.'
