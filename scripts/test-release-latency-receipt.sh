#!/usr/bin/env bash
# Deterministic self-test for scripts/release-latency-receipt.sh.
#
# The latency number is the claim GH #449 is closed on, so the script that
# produces it must be provably unable to flatter a release: it measures from
# the FIRST run of the tag (not a rerun), and it fails when the budget is
# exceeded rather than printing a number nobody checks.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
receipt="$script_dir/release-latency-receipt.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"

pass=0
fail=0

ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }

expect_field() {
    local output="$1" field="$2" expected="$3" label="$4" actual
    actual="$(grep -m1 "^$field=" <<<"$output" | cut -d= -f2-)"
    if [[ "$actual" == "$expected" ]]; then ok "$label"; else
        bad "$label (expected $field=$expected; got $actual)"
    fi
}

cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
case "$1 $2" in
  "release view")
    if [[ -z "${FAKE_PUBLISHED_AT:-}" ]]; then exit 1; fi
    printf '%s\n' "$FAKE_PUBLISHED_AT"
    ;;
  "api repos"*)
    cat "${FAKE_RUNS_JSON:?}"
    ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$tmp/bin/gh"
export GH_BIN="$tmp/bin/gh"
export RELEASE_REPO=Richards-LLC/cassy

# Two runs for the tag: the original push and a later rerun. Measuring from
# the rerun would understate the latency an operator actually experienced.
cat >"$tmp/runs.json" <<'EOF'
{"workflow_runs":[
  {"id":999,"created_at":"2026-08-20T12:30:00Z"},
  {"id":111,"created_at":"2026-08-20T12:00:00Z"}]}
EOF
export FAKE_RUNS_JSON="$tmp/runs.json"

# 1. Fast publication inside the budget.
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" v3.4.0)"
expect_field "$out" PUBLISH_LATENCY_SECONDS 250 'latency is measured from the first run of the tag'
expect_field "$out" TAG_RUN_ID 111 'receipt names the original tag run, not a rerun'
expect_field "$out" BUDGET_SECONDS 600 'default budget is the ten-minute target'
expect_field "$out" WITHIN_BUDGET true 'a fast release reports within budget'

# 2. A slow release must fail, not merely report.
set +e
slow_out="$(FAKE_PUBLISHED_AT=2026-08-20T12:21:00Z "$receipt" v3.4.0 2>&1)"
slow_status=$?
set -e
if [[ "$slow_status" -ne 0 ]]; then
    ok 'an over-budget release exits non-zero'
else
    bad 'an over-budget release exited 0'
fi
grep -qF 'over the 600s budget' <<<"$slow_out" \
    && ok 'over-budget failure names the budget' \
    || bad 'over-budget failure does not name the budget'

# 3. An explicit budget is honoured.
out="$(FAKE_PUBLISHED_AT=2026-08-20T12:21:00Z "$receipt" v3.4.0 --budget-seconds 1800)"
expect_field "$out" PUBLISH_LATENCY_SECONDS 1260 'explicit budget still reports the real latency'
expect_field "$out" WITHIN_BUDGET true 'an explicit wider budget passes'

# 4. An unpublished release cannot produce a latency receipt.
set +e
FAKE_PUBLISHED_AT= "$receipt" v3.4.0 >/dev/null 2>&1
unpublished_status=$?
set -e
if [[ "$unpublished_status" -eq 1 ]]; then
    ok 'an unpublished release fails instead of reporting a number'
else
    bad "an unpublished release exited $unpublished_status"
fi

# 5. Argument validation.
for bad_args in "3.4.0" "v3.4.0 --budget-seconds"; do
    set +e
    # shellcheck disable=SC2086
    FAKE_PUBLISHED_AT=2026-08-20T12:04:10Z "$receipt" $bad_args >/dev/null 2>&1
    status=$?
    set -e
    if [[ "$status" -eq 2 ]]; then
        ok "usage error for: $bad_args"
    else
        bad "expected usage exit 2 for: $bad_args (got $status)"
    fi
done

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
