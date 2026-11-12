#!/usr/bin/env bash
# One clock: hidden pre-PR waits, two installs, unknown and reversed endpoints.
set -euo pipefail
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/run"
cat >"$tmp/bin/gh" <<'GH'
#!/usr/bin/env bash
case "$*" in
  'release view '*) echo '2026-11-12T15:00:00Z' ;;
  *'/commits/v'*) printf '%040d\n' 1 ;;
  *'/pulls'*) echo '[{"number":42,"base":{"ref":"main"},"created_at":"2026-11-12T14:35:00Z","merged_at":"2026-11-12T14:50:00Z","merge_commit_sha":"0000000000000000000000000000000000000001"}]' ;;
  *'/releases/tags/'*) cat "$FAKE_BODY" ;;
  *) echo '{"workflow_runs":[]}' ;;
esac
GH
cat >"$tmp/bin/cas" <<'CAS'
#!/usr/bin/env bash
cat "$FAKE_CONTEXT"
CAS
chmod +x "$tmp/bin/gh" "$tmp/bin/cas"
export GH_BIN="$tmp/bin/gh" CAS_BIN="$tmp/bin/cas" FAKE_CONTEXT="$tmp/context" FAKE_BODY="$tmp/body"
printf '{"last_delivery_merged_at":"2026-11-12T12:00:00Z"}\n' >"$FAKE_CONTEXT"
printf '1794493200\n' >"$tmp/run/cut.start.epoch" # 14:20Z; pre-PR wait must count
cat >"$FAKE_BODY" <<'BODY'
<!-- cas-release-install host=soundwave installed=1794495660 -->
<!-- cas-release-install host=prowl installed=1794495720 -->
BODY
run() { "$script_dir/release-latency-receipt.sh" v9.99.1 --run-dir "$tmp/run"; }
field() { sed -n "s/^$1=//p" <<<"$out"; }
checks=0
expect() { [[ "$(field "$1")" == "$2" ]] || { echo "$1: expected $2; got $(field "$1")" >&2; exit 1; }; checks=$((checks + 1)); }
out="$(run)"
expect LAST_MERGE_TO_MAIN_SECONDS 10200
expect MAIN_TO_PUBLISHED_SECONDS 600
expect FULL_CLOCK_WITHIN_BUDGET false
[[ "$(field RELEASE_CLOCK_ROW)" == *soundwave*prowl* ]] && checks=$((checks + 1))
# Retried measurements cannot move the original cut start.
[[ "$(cat "$tmp/run/cut.start.epoch")" == 1794493200 ]] && checks=$((checks + 1))
printf '{"last_delivery_merged_at":"2026-11-12T14:20:00Z"}\n' >"$FAKE_CONTEXT"
out="$(run)"
expect LAST_MERGE_TO_MAIN_SECONDS 1800
expect FULL_CLOCK_WITHIN_BUDGET true
# A missing host or missing train merge never claims the full clock met budget.
sed -i '/host=prowl/d' "$FAKE_BODY"
out="$(run)"
expect FULL_CLOCK_WITHIN_BUDGET unavailable
printf '{}\n' >"$FAKE_CONTEXT"
out="$(run)"
expect LAST_MERGE_TO_MAIN_SECONDS unavailable
expect FULL_CLOCK_WITHIN_BUDGET unavailable
# A last merge after main is incoherent, not a negative success.
printf '{"last_delivery_merged_at":"2026-11-12T15:20:00Z"}\n' >"$FAKE_CONTEXT"
out="$(run)"
expect LAST_MERGE_TO_MAIN_SECONDS unavailable
expect FULL_CLOCK_WITHIN_BUDGET unavailable
printf '%s passed, 0 failed\n' "$checks"
