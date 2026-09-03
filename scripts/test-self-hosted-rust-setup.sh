#!/usr/bin/env bash
# Regression fixture for the shared-home self-hosted Rust setup. Two lanes must
# be able to start together without mutating the same rustup home concurrently.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture_root="$(mktemp -d "${TMPDIR:-/tmp}/cassy-rustup-setup.XXXXXX")"
trap 'rm -rf "$fixture_root"' EXIT

fake_rustup="$fixture_root/fake-rustup"
cat >"$fake_rustup" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

case "${1:-}:${2:-}" in
    toolchain:list)
        if [[ -f "$RUSTUP_HOME/installed" ]]; then
            printf '%s\n' 'stable-x86_64-unknown-linux-gnu (default)'
        fi
        ;;
    toolchain:install)
        test "${3:-}" = stable
        test "${4:-}" = --profile
        test "${5:-}" = minimal
        if ! mkdir "$RUSTUP_HOME/mutation-in-progress"; then
            echo 'rustup fixture observed concurrent mutation' >&2
            exit 42
        fi
        trap 'rmdir "$RUSTUP_HOME/mutation-in-progress"' EXIT
        sleep 0.1
        count=0
        if [[ -f "$RUSTUP_HOME/install-count" ]]; then
            count="$(<"$RUSTUP_HOME/install-count")"
        fi
        printf '%s\n' "$((count + 1))" >"$RUSTUP_HOME/install-count"
        : >"$RUSTUP_HOME/installed"
        ;;
    run:stable)
        test "${3:-}" = rustc
        test "${4:-}" = --version
        test -f "$RUSTUP_HOME/installed"
        printf '%s\n' 'rustc 1.88.0 (fixture)'
        ;;
    *)
        echo "unexpected rustup fixture invocation: $*" >&2
        exit 1
        ;;
esac
EOF
chmod 0755 "$fake_rustup"
mkdir -p "$fixture_root/rustup"

export RUSTUP_HOME="$fixture_root/rustup"
export RUSTUP="$fake_rustup"

"$repo_root/scripts/setup-cassy-actions-rust.sh" >"$fixture_root/first.log" 2>&1 &
first_pid=$!
"$repo_root/scripts/setup-cassy-actions-rust.sh" >"$fixture_root/second.log" 2>&1 &
second_pid=$!
wait "$first_pid"
wait "$second_pid"

test "$(<"$RUSTUP_HOME/install-count")" = 1
grep -q 'already installed' "$fixture_root/first.log" "$fixture_root/second.log"
printf 'PASS shared-home rustup fixture: two concurrent setup calls performed one install\n'
