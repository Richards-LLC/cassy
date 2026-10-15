#!/usr/bin/env bash
# Deterministic self-test for scripts/install-on-publish.sh (cas-f95b).
#
# Proves: a newer published release is installed with `cas update --yes
# --version`; the release notes' timer is completed once with the install
# time and the PR-opened -> installed minutes; an installed or older release
# is a no-op; a failed install is retried only after the back-off.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
installer="$script_dir/install-on-publish.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0
ok() { printf 'ok   %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf 'FAIL %s\n' "$1"; fail=$((fail + 1)); }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }

mkdir -p "$tmp/bin"
cat >"$tmp/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_LOG"
case "$1 $2" in
  "api repos/example/repo/releases/latest") printf '%s\n' "$FAKE_LATEST" ;;
  "api repos/example/repo/releases/tags/"*) cat "$FAKE_BODY" ;;
  "release edit")
    while (($#)); do [[ "$1" == --notes-file ]] && cp "$2" "$FAKE_BODY"; shift; done ;;
  *) echo "unexpected fake gh invocation: $*" >&2; exit 2 ;;
esac
EOF
cat >"$tmp/bin/cas" <<'EOF'
#!/usr/bin/env bash
if [[ "$1" == --version ]]; then echo "cas $(cat "$FAKE_INSTALLED") (abc 2026-10-15)"; exit 0; fi
printf 'cas %s\n' "$*" >>"$FAKE_LOG"
[[ "${FAKE_UPDATE_FAILS:-}" == 1 ]] && exit 1
[[ "$1 $2 $3" == "update --yes --version" ]] && printf '%s\n' "$4" >"$FAKE_INSTALLED"
EOF
chmod +x "$tmp/bin/gh" "$tmp/bin/cas"

export GH_BIN="$tmp/bin/gh" CAS_BIN="$tmp/bin/cas" CAS_RELEASE_REPO=example/repo
export FAKE_LOG="$tmp/log" FAKE_INSTALLED="$tmp/installed" FAKE_BODY="$tmp/body.md"
export XDG_STATE_HOME="$tmp/state" RELEASE_INSTALL_HOST=testhost
opened=1760500000
published=$((opened + 20 * 60))
installed=$((opened + 24 * 60))
cat >"$FAKE_BODY" <<EOF
- One shipped change.

---

**Release timer:** PR #4242 opened x → published y: 20 min. Installed: pending.
<!-- cas-release-timer pr=4242 opened=$opened published=$published -->
EOF

# 1. A newer release installs and completes the timer.
echo 9.99.1 >"$FAKE_INSTALLED"
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 RELEASE_INSTALL_NOW="$installed" "$installer" --once >"$tmp/out"
check 'grep -q "^cas update --yes --version 9.99.2$" "$FAKE_LOG"' 'a newer release runs cas update for that version'
check '[[ "$(cat "$FAKE_INSTALLED")" == 9.99.2 ]]' 'the host converges on the published version'
check 'grep -q "Installed on testhost .*PR #4242 opened → installed: 24 min" "$FAKE_BODY"' 'the release notes print the PR-opened -> installed timer'
check 'grep -q "<!-- cas-release-timer pr=4242 opened=$opened published=$published installed=$installed -->" "$FAKE_BODY"' 'the timer marker records the install time'
check '! grep -q "Installed: pending" "$FAKE_BODY"' 'the pending timer is replaced'

# 2. An installed release is a no-op: no update, no notes edit.
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 "$installer" --once >/dev/null
check '! grep -q "update\|release edit" "$FAKE_LOG"' 'an installed release does nothing'

# 3. A second host leaves the completed timer alone.
echo 9.99.1 >"$FAKE_INSTALLED"
cp "$FAKE_BODY" "$tmp/before.md"
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.2 RELEASE_INSTALL_HOST=other "$installer" --once >/dev/null
check 'cmp -s "$FAKE_BODY" "$tmp/before.md" && ! grep -q "release edit" "$FAKE_LOG"' 'a completed timer is not overwritten'

# 4. A failed install is retried only after the back-off.
echo 9.99.2 >"$FAKE_INSTALLED"
: >"$FAKE_LOG"
set +e
FAKE_LATEST=v9.99.3 FAKE_UPDATE_FAILS=1 RELEASE_INSTALL_NOW=2000 "$installer" --once >/dev/null
status=$?
set -e
check '[[ $status -ne 0 ]] && grep -q "^cas update --yes --version 9.99.3$" "$FAKE_LOG"' 'a failed install exits non-zero'
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.3 FAKE_UPDATE_FAILS=1 RELEASE_INSTALL_NOW=2100 "$installer" --once >/dev/null || true
check '! grep -q "update" "$FAKE_LOG"' 'a failed version is not retried inside the back-off'
: >"$FAKE_LOG"
FAKE_LATEST=v9.99.3 RELEASE_INSTALL_NOW=2700 "$installer" --once >/dev/null
check 'grep -q "^cas update --yes --version 9.99.3$" "$FAKE_LOG" && [[ "$(cat "$FAKE_INSTALLED")" == 9.99.3 ]]' 'the install is retried after the back-off'

# 5. A lookup failure is quiet and harmless.
: >"$FAKE_LOG"
FAKE_LATEST=not-a-version "$installer" --once >/dev/null
check '! grep -q "update" "$FAKE_LOG"' 'a malformed latest tag installs nothing'

printf '\n%s passed, %s failed\n' "$pass" "$fail"
test "$fail" -eq 0
