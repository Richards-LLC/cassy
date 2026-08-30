#!/usr/bin/env bash
# Deterministic self-test for scripts/codemap-latency-receipt.sh.
set -uo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipt="$script_dir/codemap-latency-receipt.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0

ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

expect_field() {
    output="$1"
    field="$2"
    expected="$3"
    label="$4"
    actual="$(grep -m1 "^$field=" <<<"$output" | cut -d= -f2-)"
    if [[ "$actual" == "$expected" ]]; then
        ok "$label"
    else
        bad "$label (expected $field=$expected; got $actual)"
    fi
}

cat >"$tmp/fake-clock" <<'EOF'
#!/usr/bin/env bash
state="${CLOCK_STATE:?}"
step="${CLOCK_STEP:-1}"
count=0
if [[ -f "$state" ]]; then count="$(<"$state")"; fi
printf '%s\n' $((count + step)) >"$state"
printf '%s\n' $((count + step))
EOF
chmod +x "$tmp/fake-clock"

cat >"$tmp/fake-cas" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  "codemap status")
    printf '%s\n' 'CODEMAP.md: /tmp/project/.claude/CODEMAP.md' '  Status: up to date'
    ;;
  "knowledge build")
    printf '%s\n' 'knowledge build completed'
    ;;
  *)
    echo "unexpected fake cas invocation: $*" >&2
    exit 2
    ;;
esac
EOF
chmod +x "$tmp/fake-cas"

cat >"$tmp/fake-gh" <<'EOF'
#!/usr/bin/env bash
if [[ "$1 $2" == 'run view' ]]; then
    cat <<'JSON'
{"createdAt":"2026-08-30T12:00:00Z","url":"https://github.com/example/repo/actions/runs/123","jobs":[
  {"name":"Fast Validation","startedAt":"2026-08-30T12:00:03Z","completedAt":"2026-08-30T12:00:05Z","conclusion":"success"},
  {"name":"macOS Check","startedAt":"2026-08-30T12:00:04Z","completedAt":"2026-08-30T12:00:09Z","conclusion":"success"},
  {"name":"Clippy (advisory)","startedAt":"2026-08-30T12:00:03Z","completedAt":"2026-08-30T12:01:00Z","conclusion":"skipped"}
]}
JSON
else
    echo "unexpected fake gh invocation: $*" >&2
    exit 2
fi
EOF
chmod +x "$tmp/fake-gh"

export CAS_BIN="$tmp/fake-cas"
export GH_BIN="$tmp/fake-gh"
export CLOCK_STATE="$tmp/clock-state"
export CODEMAP_LATENCY_CLOCK="$tmp/fake-clock"

artifact="$tmp/receipt.env"
out="$(CLOCK_STEP=1 "$receipt" --artifact "$artifact" --github-run-id 123 --github-repo example/repo)"
expect_field "$out" CODEMAP_RENDER_STATUS identical 'a no-op render is recognized as identical'
expect_field "$out" NO_CONTENT_CHANGE_PRESERVED true 'identical render preserves the no-content rule'
expect_field "$out" KNOWLEDGE_BUILD_BUDGET_SECONDS 90 'knowledge build defaults to a 90-second bound'
expect_field "$out" KNOWLEDGE_BUILD_WITHIN_BUDGET true 'knowledge build is within its bound'
expect_field "$out" AGENT_CONTROLLED_TOTAL_SECONDS 4 'agent-controlled phases exclude docs and queue time'
expect_field "$out" AGENT_CONTROLLED_WITHIN_BUDGET true 'agent-controlled total is within five minutes'
expect_field "$out" DOCS_ONLY_REQUIRED_COMPUTE_SECONDS 5 'required docs path uses the slower required context'
expect_field "$out" DOCS_ONLY_REQUIRED_COMPUTE_SOURCE github-required-jobs 'required docs timing identifies its source'
expect_field "$out" DOCS_ONLY_REQUIRED_COMPUTE_WITHIN_BUDGET true 'required docs path is within one minute'
expect_field "$out" GITHUB_QUEUE_SECONDS 3 'GitHub queue delay starts at workflow creation and ends at first runner'
expect_field "$out" GITHUB_QUEUE_STATUS measured 'GitHub queue timing is measured separately'
if cmp -s "$artifact" <(printf '%s\n' "$out" | sed '/^ARTIFACT_PATH=/d'); then
    ok 'artifact is byte-for-byte the emitted receipt'
else
    bad 'artifact differs from emitted receipt'
fi

# A changed candidate must fail before CODEMAP.md can be touched.
printf '%s\n' 'changed render' >"$tmp/changed-CODEMAP.md"
before_hash="$(shasum -a 256 .claude/CODEMAP.md)"
set +e
CLOCK_STATE="$tmp/changed-clock-state" CLOCK_STEP=1 "$receipt" --rendered-path "$tmp/changed-CODEMAP.md" >"$tmp/changed.out" 2>&1
changed_status=$?
set -e
after_hash="$(shasum -a 256 .claude/CODEMAP.md)"
if [[ "$changed_status" -ne 0 ]]; then ok 'changed render exits non-zero'; else bad 'changed render exited zero'; fi
if [[ "$before_hash" == "$after_hash" ]]; then ok 'changed render never modifies CODEMAP.md'; else bad 'changed render modified CODEMAP.md'; fi

# Four local agent phases at 76 seconds each exceed 300 seconds while the
# bounded knowledge phase itself remains under 90 seconds.
set +e
CLOCK_STATE="$tmp/slow-clock-state" CLOCK_STEP=76 "$receipt" --github-run-id 123 --github-repo example/repo >"$tmp/slow.out" 2>&1
slow_status=$?
set -e
slow_output="$(<"$tmp/slow.out")"
if [[ "$slow_status" -ne 0 ]]; then ok 'over-budget agent-controlled work exits non-zero'; else bad 'over-budget agent-controlled work exited zero'; fi
expect_field "$slow_output" AGENT_CONTROLLED_TOTAL_SECONDS 304 'agent total is the sum of the four local phases'
expect_field "$slow_output" AGENT_CONTROLLED_WITHIN_BUDGET false 'agent budget failure is explicit'
expect_field "$slow_output" KNOWLEDGE_BUILD_WITHIN_BUDGET true 'knowledge bound remains independent of agent total'

set +e
CODEMAP_AGENT_BUDGET_SECONDS=0 "$receipt" >/dev/null 2>&1
invalid_status=$?
set -e
if [[ "$invalid_status" -eq 2 ]]; then ok 'invalid budget is rejected'; else bad "invalid budget exited $invalid_status"; fi

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
