#!/usr/bin/env bash
# Regression fixture for the shared-home self-hosted Rust setup. Two lanes must
# be able to start together without mutating the same rustup home concurrently.
set -euo pipefail

if [[ "$(uname -s)" == Darwin ]]; then
    printf 'SKIP shared-home rustup fixture on Darwin: Linux flock is required to serialize the shared runner toolchain\n'
    exit 0
fi

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
        printf '%s\n' "$BASHPID" >"$RUSTUP_HOME/install-entered"
        read -r -t 10 release <>"$RUSTUP_HOME/install-release"
        count=0
        if [[ -f "$RUSTUP_HOME/install-count" ]]; then
            count="$(<"$RUSTUP_HOME/install-count")"
        fi
        printf '%s\n' "$((count + 1))" >"$RUSTUP_HOME/install-count"
        : >"$RUSTUP_HOME/installed"
        ;;
    target:list)
        test "$*" = 'target list --toolchain stable --installed'
        test -f "$RUSTUP_HOME/installed"
        printf '%s\n' 'x86_64-unknown-linux-gnu'
        ;;
    run:stable)
        test -f "$RUSTUP_HOME/installed"
        case "${3:-}:${4:-}" in
            rustc:--version) printf '%s\n' 'rustc 1.88.0 (fixture)' ;;
            rustc:-vV) printf '%s\n' 'rustc 1.88.0 (fixture)' 'host: x86_64-unknown-linux-gnu' ;;
            rustc:--print)
                test "${5:-}" = target-libdir
                mkdir -p "$RUSTUP_HOME/lib"
                : >"$RUSTUP_HOME/lib/libstd-fixture.rlib"
                printf '%s\n' "$RUSTUP_HOME/lib"
                ;;
            cargo:--version) printf '%s\n' 'cargo 1.88.0 (fixture)' ;;
            *) echo "unexpected run fixture invocation: $*" >&2; exit 1 ;;
        esac
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
export TEST_REAL_FLOCK="$(command -v flock)"
mkdir -p "$fixture_root/bin"
mkfifo "$RUSTUP_HOME/install-entered" "$RUSTUP_HOME/install-release" "$RUSTUP_HOME/lock-contended"
cat >"$fixture_root/bin/flock" <<'EOF'
#!/usr/bin/env bash
if [[ "$*" == "-x 9" ]]; then
    if "$TEST_REAL_FLOCK" -n -x 9; then exit 0; fi
    printf '%s\n' "$BASHPID" >"$RUSTUP_HOME/lock-contended"
fi
exec "$TEST_REAL_FLOCK" "$@"
EOF
chmod +x "$fixture_root/bin/flock"
export PATH="$fixture_root/bin:$PATH"
"$repo_root/scripts/setup-cassy-actions-rust.sh" >"$fixture_root/first.log" 2>&1 &
first_pid=$!
read -r -t 10 installer <>"$RUSTUP_HOME/install-entered"
"$repo_root/scripts/setup-cassy-actions-rust.sh" >"$fixture_root/second.log" 2>&1 &
second_pid=$!
read -r -t 10 contender <>"$RUSTUP_HOME/lock-contended"
printf 'release\n' >"$RUSTUP_HOME/install-release"
wait "$first_pid"
wait "$second_pid"

test "$(<"$RUSTUP_HOME/install-count")" = 1
printf 'PASS shared-home rustup fixture: two concurrent setup calls performed one install\n'
