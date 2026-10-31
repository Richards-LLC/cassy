#!/usr/bin/env bash
# Fixture tests for the release-publication state machine. No network access.
set -euo pipefail
# Hook callers export Git repository paths; keep the fixture isolated.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars)

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=scripts/release-portable.sh
source "$script_dir/release-portable.sh"
release_portable_define_sha256sum
receipt="$script_dir/release-published-receipt.sh"
template="$script_dir/../docs/release-notes/runtime-release-template.md"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

fixture="$tmpdir/fixture"
mkdir -p "$fixture/assets" "$fixture/repo/scripts"
# Exercise the real digest and latency producers against a local tag remote.
for helper in release-published-receipt.sh release-latency-receipt.sh release-portable.sh release-interventions.py release-metrics.py release-stage-budgets.json; do
    cp "$script_dir/$helper" "$fixture/repo/scripts/"
done
receipt="$fixture/repo/scripts/release-published-receipt.sh"
git init --quiet --bare "$fixture/origin.git"
git init --quiet "$fixture/repo"
git -C "$fixture/repo" config user.email fixture@example.test
git -C "$fixture/repo" config user.name Fixture
git -C "$fixture/repo" config core.hooksPath /dev/null
git -C "$fixture/repo" commit --quiet --allow-empty -m previous
git -C "$fixture/repo" tag v2.69.0
git -C "$fixture/repo" commit --quiet --allow-empty -m release
export FIXTURE_RELEASE_SHA="$(git -C "$fixture/repo" rev-parse HEAD)"
git -C "$fixture/repo" tag v2.69.1
git -C "$fixture/repo" remote add origin "$fixture/origin.git"
git -C "$fixture/repo" push --quiet origin --tags
git -C "$fixture/repo" tag -d v2.69.1 >/dev/null
export CAS_RELEASE_ARTIFACTS_ROOT="$tmpdir/artifacts"
unset CAS_RELEASE_TRAIN_RUN_DIR CAS_RELEASE_RUN_DIR
export FIXTURE_LEDGER_LOG="$fixture/ledger.log" FIXTURE_LEDGER_RECEIPT="$fixture/ledger.receipt"
: >"$FIXTURE_LEDGER_LOG"
cat >"$tmpdir/cas" <<'EOF_CAS'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1 $2 $3" == 'ledger record-release --receipt' && "$#" == 4 ]]
printf '%s\n' "$1 $2 $3" >>"$FIXTURE_LEDGER_LOG"
cp "$4" "$FIXTURE_LEDGER_RECEIPT"
# The freshly published remote tag must be available to the CLI's Git metrics.
git rev-parse --verify 'v2.69.1^{commit}' >/dev/null
printf 'Recorded release v2.69.1.\n'
exit "${FIXTURE_LEDGER_STATUS:-0}"
EOF_CAS
chmod +x "$tmpdir/cas"
export CAS_BIN="$tmpdir/cas"
printf 'linux published bytes\n' >"$fixture/assets/cas-x86_64-unknown-linux-gnu.tar.gz"
printf 'macos published bytes\n' >"$fixture/assets/cas-aarch64-apple-darwin.tar.gz"
linux_sha="$(sha256sum "$fixture/assets/cas-x86_64-unknown-linux-gnu.tar.gz" | awk '{print $1}')"
macos_sha="$(sha256sum "$fixture/assets/cas-aarch64-apple-darwin.tar.gz" | awk '{print $1}')"

fake_gh="$tmpdir/gh"
cat >"$fake_gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
  "release view")
    if [[ "$*" == *'--json publishedAt --jq'* ]]; then
      printf '2026-08-15T00:57:26Z\n'
    else
      cat "$FIXTURE/release.json"
    fi
    ;;
  "api repos/"*)
    case "$2" in
      */commits/*/pulls)
        printf '[{"number":42,"base":{"ref":"main"},"merged_at":"2026-08-15T00:48:26Z","merge_commit_sha":"%s"}]\n' "$FIXTURE_RELEASE_SHA" ;;
      */commits/*) printf '%s\n' "$FIXTURE_RELEASE_SHA" ;;
      *branch=main*)
        printf '{"workflow_runs":[{"id":123,"event":"push","head_branch":"main","head_sha":"%s","created_at":"2026-08-15T00:48:30Z"}]}\n' "$FIXTURE_RELEASE_SHA" ;;
      *) printf '{"workflow_runs":[]}\n' ;;
    esac
    ;;
  "release download")
    pattern=""
    directory=""
    while [[ "$#" -gt 0 ]]; do
      case "$1" in
        --pattern) pattern="$2"; shift 2 ;;
        --dir) directory="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    cp "$FIXTURE/assets/$pattern" "$directory/$pattern"
    ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
chmod +x "$fake_gh"

write_release() {
  cat >"$fixture/release.json" <<EOF
{"isDraft":false,"publishedAt":"2026-08-15T00:57:26Z","assets":$1}
EOF
}

assert_no_digest_placeholders() {
  local candidate="$1"

  if grep -qF '{{LINUX_SHA256}}' "$candidate" || grep -qF '{{MACOS_SHA256}}' "$candidate"; then
    echo 'FAIL receipt left a digest placeholder in the draft' >&2
    return 1
  fi
}

# A release object alone is insufficient: this pins the exact half-uploaded
# state that used to let a local digest reach the announcement.
write_release "[{\"name\":\"cas-x86_64-unknown-linux-gnu.tar.gz\",\"digest\":\"sha256:$linux_sha\"}]"
set +e
partial_output="$(FIXTURE="$fixture" GH_BIN="$fake_gh" "$receipt" v2.69.1 2>&1)"
partial_status=$?
set -e
test "$partial_status" -ne 0
grep -qF 'release v2.69.1 is partial; required asset cas-aarch64-apple-darwin.tar.gz' <<<"$partial_output"
echo 'ok   partial published release fails closed before announcement fields'

write_release "[{\"name\":\"cas-x86_64-unknown-linux-gnu.tar.gz\",\"digest\":\"sha256:$(printf '0%.0s' {1..64})\"},{\"name\":\"cas-aarch64-apple-darwin.tar.gz\",\"digest\":\"sha256:$macos_sha\"}]"
set +e
mismatch_output="$(FIXTURE="$fixture" GH_BIN="$fake_gh" "$receipt" v2.69.1 2>&1)"
mismatch_status=$?
set -e
test "$mismatch_status" -ne 0
grep -qF 'does not match GitHub digest' <<<"$mismatch_output"
echo 'ok   mismatched downloaded bytes fail closed'
test ! -s "$FIXTURE_LEDGER_LOG"

write_release "[{\"name\":\"cas-x86_64-unknown-linux-gnu.tar.gz\",\"digest\":\"sha256:$linux_sha\"},{\"name\":\"cas-aarch64-apple-darwin.tar.gz\",\"digest\":\"sha256:$macos_sha\"}]"
draft="$fixture/draft.md"
# One existing success case covers recording and its best-effort failure path.
for ledger_status in 0 23; do
  : >"$FIXTURE_LEDGER_LOG"
  rm -f "$FIXTURE_LEDGER_RECEIPT"
  printf 'Linux {{LINUX_SHA256}} / {{LINUX_SHA256}}; macOS {{MACOS_SHA256}} / {{MACOS_SHA256}}\n' >"$draft"
  success_output="$(FIXTURE="$fixture" GH_BIN="$fake_gh" FIXTURE_LEDGER_STATUS="$ledger_status" \
    "$receipt" v2.69.1 --write-draft "$draft" 2>"$fixture/receipt.err")"
  grep -qFx 'TAG=v2.69.1' <<<"$success_output"
  grep -qFx "LINUX_SHA256=$linux_sha" <<<"$success_output"
  grep -qFx "MACOS_SHA256=$macos_sha" <<<"$success_output"
  grep -qF "$linux_sha" "$draft"
  grep -qF "$macos_sha" "$draft"
  test "$(grep -oF "$linux_sha" "$draft" | wc -l)" -eq 2
  test "$(grep -oF "$macos_sha" "$draft" | wc -l)" -eq 2
  assert_no_digest_placeholders "$draft"
  if [[ "$(wc -l <"$FIXTURE_LEDGER_LOG")" -ne 1 ]]; then
    echo 'FAIL published receipt did not invoke release-ledger recording exactly once' >&2
    cat "$fixture/receipt.err" >&2
    exit 1
  fi
  grep -qFx 'ledger record-release --receipt' "$FIXTURE_LEDGER_LOG"
  grep -qFx 'TAG=v2.69.1' "$FIXTURE_LEDGER_RECEIPT"
  grep -qFx 'PUBLISHED_AT=2026-08-15T00:57:26Z' "$FIXTURE_LEDGER_RECEIPT"
  grep -qFx 'MAIN_LANDED_AT=2026-08-15T00:48:26Z' "$FIXTURE_LEDGER_RECEIPT"
  grep -qFx 'PUBLISH_LATENCY_SECONDS=540' "$FIXTURE_LEDGER_RECEIPT"
  ! grep -q '^Recorded release' <<<"$success_output"
done
echo 'ok   complete release records real publication timings once, even when ledger recording fails, and writes fresh digests'

placeholder_draft="$fixture/draft-with-placeholder.md"
printf 'Linux {{LINUX_SHA256}} / verified macOS digest\n' >"$placeholder_draft"
set +e
assert_no_digest_placeholders "$placeholder_draft" >/dev/null 2>&1
placeholder_status=$?
set -e
test "$placeholder_status" -ne 0
echo 'ok   residual digest placeholder is rejected'

test "$(grep -oF '{{LINUX_SHA256}}' "$template" | wc -l)" -eq 2
test "$(grep -oF '{{MACOS_SHA256}}' "$template" | wc -l)" -eq 2
echo 'ok   runtime draft template exposes only receipt-fillable digest placeholders'
