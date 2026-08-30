#!/usr/bin/env bash
# Deterministic self-test for scripts/find-release-prebuild.sh.
#
# This lookup decides whether a tag publishes prebuilt bytes or compiles from
# scratch. It must adopt only a *complete* prebuild of the *exact* commit, wait
# for an in-flight matching run for a bounded period, and decline — never fail —
# on every degraded input, because declining costs minutes while failing costs
# the release.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lookup="$script_dir/find-release-prebuild.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

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

# A fake `gh api` whose two responses are supplied as files. Anything else it
# is asked for is an error, so an unexpected API call cannot pass silently.
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" != api ]]; then
  echo "unexpected fake gh invocation: $*" >&2
  exit 2
fi
case "$2" in
  *"/actions/workflows/"*"/runs?"*)
    if [[ "$2" == *"status=success"* ]]; then
      echo "unexpected status-restricted prebuild run query: $2" >&2
      exit 2
    fi
    if [[ -n "${FAKE_RUNS_STATUS:-}" ]]; then exit "$FAKE_RUNS_STATUS"; fi
    if [[ -n "${FAKE_RUNS_SEQUENCE_DIR:-}" ]]; then
      count_file="$FAKE_RUNS_SEQUENCE_DIR/count"
      count=0
      [[ -f "$count_file" ]] && count="$(<"$count_file")"
      count=$((count + 1))
      printf '%s\n' "$count" >"$count_file"
      sequence_file="$FAKE_RUNS_SEQUENCE_DIR/$count.json"
      [[ -f "$sequence_file" ]] || sequence_file="$FAKE_RUNS_SEQUENCE_DIR/last.json"
      cat "$sequence_file"
    else
      cat "${FAKE_RUNS_JSON:?}"
    fi
    ;;
  *"/actions/runs/"*"/artifacts?"*)
    run_id="${2#*/actions/runs/}"
    run_id="${run_id%%/artifacts*}"
    file="${FAKE_ARTIFACTS_DIR:?}/$run_id.json"
    if [[ ! -f "$file" ]]; then exit 1; fi
    cat "$file"
    ;;
  *)
    echo "unexpected fake gh api path: $2" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$tmp/bin/gh"
export GH_BIN="$tmp/bin/gh"
export GITHUB_REPOSITORY=Richards-LLC/cassy
mkdir -p "$tmp/artifacts"

both_live='{"artifacts":[
  {"name":"cas-x86_64-unknown-linux-gnu","expired":false},
  {"name":"cas-aarch64-apple-darwin","expired":false}]}'
linux_only='{"artifacts":[{"name":"cas-x86_64-unknown-linux-gnu","expired":false}]}'
macos_expired='{"artifacts":[
  {"name":"cas-x86_64-unknown-linux-gnu","expired":false},
  {"name":"cas-aarch64-apple-darwin","expired":true}]}'

echo '{"workflow_runs":[{"id":111,"status":"completed","conclusion":"success"}]}' >"$tmp/runs.json"
export FAKE_RUNS_JSON="$tmp/runs.json"
export FAKE_ARTIFACTS_DIR="$tmp/artifacts"

# 1. Complete prebuild -> adopt it.
printf '%s' "$both_live" >"$tmp/artifacts/111.json"
out="$("$lookup" deadbeef)"
expect_field "$out" found true 'a complete prebuild for the commit is adopted'
expect_field "$out" run-id 111 'adoption names the prebuild run'

# 2. Linux built, macOS missing -> decline (never publish half a release).
printf '%s' "$linux_only" >"$tmp/artifacts/111.json"
out="$("$lookup" deadbeef)"
expect_field "$out" found false 'a partial prebuild is not adopted'

# 3. Artifact aged out of retention -> decline.
printf '%s' "$macos_expired" >"$tmp/artifacts/111.json"
out="$("$lookup" deadbeef)"
expect_field "$out" found false 'an expired artifact is not adopted'

# 4. No prebuild run for the commit -> decline.
echo '{"workflow_runs":[]}' >"$tmp/runs.json"
out="$("$lookup" deadbeef)"
expect_field "$out" found false 'a commit with no prebuild falls back to building'

# 5. Newest incomplete run must not mask an older complete one.
echo '{"workflow_runs":[
  {"id":222,"status":"completed","conclusion":"failure"},
  {"id":111,"status":"completed","conclusion":"success"}]}' >"$tmp/runs.json"
printf '%s' "$linux_only" >"$tmp/artifacts/222.json"
printf '%s' "$both_live" >"$tmp/artifacts/111.json"
out="$("$lookup" deadbeef)"
expect_field "$out" found true 'a complete older prebuild is still adopted'
expect_field "$out" run-id 111 'adoption skips the incomplete run'

# 6. API outage -> decline, exit 0. The release must still be able to ship.
out="$(FAKE_RUNS_STATUS=1 "$lookup" deadbeef)"
expect_field "$out" found false 'an API failure declines instead of failing the release'

# 7. A tag racing an in-flight prebuild waits, then adopts its completed assets.
mkdir -p "$tmp/runs-sequence"
echo '{"workflow_runs":[{"id":333,"status":"in_progress","conclusion":null}]}' \
  >"$tmp/runs-sequence/1.json"
echo '{"workflow_runs":[{"id":333,"status":"completed","conclusion":"success"}]}' \
  >"$tmp/runs-sequence/2.json"
printf '%s' "$both_live" >"$tmp/artifacts/333.json"
rm -f "$tmp/runs-sequence/count"
export FAKE_RUNS_SEQUENCE_DIR="$tmp/runs-sequence"
out="$(RELEASE_PREBUILD_WAIT_SECONDS=1 RELEASE_PREBUILD_POLL_SECONDS=0 "$lookup" racecommit)"
expect_field "$out" found true 'an in-flight prebuild is awaited and adopted'
expect_field "$out" run-id 333 'the completed racing prebuild run is adopted'
unset FAKE_RUNS_SEQUENCE_DIR

# 8. A prebuild that remains in flight past the bound falls back with a warning.
echo '{"workflow_runs":[{"id":444,"status":"queued","conclusion":null}]}' \
  >"$tmp/runs.json"
out="$(RELEASE_PREBUILD_WAIT_SECONDS=0 "$lookup" timeoutcommit 2>&1)"
expect_field "$out" found false 'an in-flight prebuild times out to the cold build'
if grep -qF 'falling back to the cold release build' <<<"$out"; then
  printf 'ok   an in-flight timeout emits a prominent cold-build warning\n'
  pass=$((pass + 1))
else
  printf 'FAIL in-flight timeout omitted its cold-build warning\n'
  fail=$((fail + 1))
fi

# 9. Missing inputs decline rather than crash the workflow step.
out="$(GITHUB_REPOSITORY= "$lookup" deadbeef)"
expect_field "$out" found false 'an unset repository declines'
out="$(GITHUB_SHA= "$lookup")"
expect_field "$out" found false 'an unset commit declines'

# Every path exits 0.
echo '{"workflow_runs":[]}' >"$tmp/runs.json"
for scenario in 'deadbeef'; do
    if "$lookup" "$scenario" >/dev/null; then
        printf 'ok   lookup exits 0 for %s\n' "$scenario"
        pass=$((pass + 1))
    else
        printf 'FAIL lookup exited non-zero for %s\n' "$scenario"
        fail=$((fail + 1))
    fi
done

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
