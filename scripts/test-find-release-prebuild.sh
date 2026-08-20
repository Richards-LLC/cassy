#!/usr/bin/env bash
# Deterministic self-test for scripts/find-release-prebuild.sh.
#
# This lookup decides whether a tag publishes prebuilt bytes or compiles from
# scratch. It must adopt only a *complete* prebuild of the *exact* commit, and
# it must decline — never fail — on every degraded input, because declining
# costs minutes while failing costs the release.
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
    if [[ -n "${FAKE_RUNS_STATUS:-}" ]]; then exit "$FAKE_RUNS_STATUS"; fi
    cat "${FAKE_RUNS_JSON:?}"
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

echo '{"workflow_runs":[{"id":111}]}' >"$tmp/runs.json"
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
echo '{"workflow_runs":[{"id":222},{"id":111}]}' >"$tmp/runs.json"
printf '%s' "$linux_only" >"$tmp/artifacts/222.json"
printf '%s' "$both_live" >"$tmp/artifacts/111.json"
out="$("$lookup" deadbeef)"
expect_field "$out" found true 'a complete older prebuild is still adopted'
expect_field "$out" run-id 111 'adoption skips the incomplete run'

# 6. API outage -> decline, exit 0. The release must still be able to ship.
out="$(FAKE_RUNS_STATUS=1 "$lookup" deadbeef)"
expect_field "$out" found false 'an API failure declines instead of failing the release'

# 7. Missing inputs decline rather than crash the workflow step.
out="$(GITHUB_REPOSITORY= "$lookup" deadbeef)"
expect_field "$out" found false 'an unset repository declines'
out="$(GITHUB_SHA= "$lookup")"
expect_field "$out" found false 'an unset commit declines'

# Every path exits 0.
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
