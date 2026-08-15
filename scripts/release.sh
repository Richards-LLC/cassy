#!/usr/bin/env bash
#
# Local release audit and tag-push script for CAS.
#
# GitHub's tag-triggered Release workflow is the sole normal publisher. This
# script builds local audit evidence and pushes the annotated tag that starts
# that workflow; its dist/local-audit archives are never the shipped bytes and
# must never supply an announced digest. Use release-published-receipt.sh only
# after the workflow has published both assets.
#
# A deliberately loud manual failover remains for a disabled or unavailable CI
# workflow. It deliberately competes with the tag-triggered workflow, so use
# it only after disabling/cancelling that workflow or when its runners cannot
# publish. That exceptional path still requires the published-release receipt
# command before any digest is announced.
#
# Prerequisites:
#   - Rust toolchain (rustup)
#   - cargo-zigbuild: cargo install cargo-zigbuild
#   - gh CLI (only for --manual-publish)
#   - Environment variables: CAS_POSTHOG_API_KEY, CAS_SENTRY_DSN
#
# Usage:
#   ./scripts/release.sh
#   ./scripts/release.sh --manual-publish --acknowledge-workflow-conflict

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

HOST_OS="$(uname -s)"
TARGETS=("x86_64-unknown-linux-gnu")
if [[ "$HOST_OS" == "Darwin" ]]; then
    TARGETS=("aarch64-apple-darwin" "x86_64-unknown-linux-gnu")
fi

DIST_DIR="$REPO_ROOT/dist/local-audit"
MANUAL_PUBLISH=false
ACKNOWLEDGED_CONFLICT=false

for arg in "$@"; do
    case "$arg" in
        --manual-publish) MANUAL_PUBLISH=true ;;
        --acknowledge-workflow-conflict) ACKNOWLEDGED_CONFLICT=true ;;
        -h|--help)
            cat <<'EOF'
Usage: scripts/release.sh [--manual-publish --acknowledge-workflow-conflict]

Build local audit archives and push the annotated tag. The tag-triggered
GitHub Release workflow creates the normal published release.

  --manual-publish
      Emergency failover only: create a release from local audit archives
      after pushing the tag. Use only while the workflow is disabled or
      unavailable; it deliberately conflicts with the normal CI publisher.
  --acknowledge-workflow-conflict
      Required with --manual-publish. Published digests must still come from
      scripts/release-published-receipt.sh, never from dist/local-audit.
EOF
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            exit 1
            ;;
    esac
done

if "$MANUAL_PUBLISH" && ! "$ACKNOWLEDGED_CONFLICT"; then
    echo "error: --manual-publish requires --acknowledge-workflow-conflict" >&2
    exit 2
fi
if ! "$MANUAL_PUBLISH" && "$ACKNOWLEDGED_CONFLICT"; then
    echo "error: --acknowledge-workflow-conflict is only valid with --manual-publish" >&2
    exit 2
fi

# Reject unsupported host/target combinations before bootstrapping toolchains
# or compiling, rather than failing deep in a native compiler invocation.
./scripts/check-release-host.sh "$HOST_OS" "${TARGETS[@]}"

if [ -f "$REPO_ROOT/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$REPO_ROOT/.env"
    set +a
fi

VERSION="$(grep -m1 '^version = "' cas-cli/Cargo.toml | sed -E 's/^version = "([^"]+)"/\1/')"
TAG="v$VERSION"

echo "=== CAS Local Release Audit ==="
echo "Version:  $VERSION"
echo "Tag:      $TAG"
echo "Targets:  ${TARGETS[*]}"
echo "Output:   $DIST_DIR (audit evidence only; not shipped bytes)"
echo ""

if [ -z "${CAS_POSTHOG_API_KEY:-}" ]; then
    echo "error: CAS_POSTHOG_API_KEY is not set" >&2
    exit 1
fi
if [ -z "${CAS_SENTRY_DSN:-}" ]; then
    echo "warning: CAS_SENTRY_DSN is not set — crash reporting will be disabled in this build"
fi
if ! command -v cargo-zigbuild &>/dev/null; then
    echo "error: cargo-zigbuild not found. Install with: cargo install cargo-zigbuild" >&2
    exit 1
fi
if "$MANUAL_PUBLISH" && ! command -v gh &>/dev/null; then
    echo "error: gh CLI not found. Install with: brew install gh" >&2
    exit 1
fi

ensure_release_tag() {
    if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
        echo "Creating annotated tag $TAG on HEAD before release audit..."
        git tag -a "$TAG" -m "$TAG"
    fi
    ./scripts/check-release-preflight.sh "$TAG"
}

# A registered migration changes the doctor/status component snapshots, while
# scoped release suites do not build that integration test.
./scripts/check-release-migration-snapshots.sh

if [ ! -x ".context/zig/zig" ]; then
    echo "Bootstrapping Zig..."
    ./scripts/bootstrap-zig.sh
fi
export ZIG="$REPO_ROOT/.context/zig/zig"
export PATH="$REPO_ROOT/.context/zig:$PATH"
echo "Zig: $(zig version)"

ensure_release_tag
git submodule update --init --recursive

INSTALLED_TARGETS="$(rustup target list --installed)"
for target in "${TARGETS[@]}"; do
    if ! grep -q "^${target}$" <<<"$INSTALLED_TARGETS"; then
        echo "Installing Rust target: $target"
        rustup target add "$target"
    fi
done

rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

for target in "${TARGETS[@]}"; do
    echo ""
    echo "=== Building $target ==="
    if [[ "$target" == *"linux"* ]]; then
        cargo clean --release --target "$target"
        CFLAGS_x86_64_unknown_linux_gnu="-march=x86_64" \
        CXXFLAGS_x86_64_unknown_linux_gnu="-march=x86_64" \
            cargo zigbuild -p cas --release --target "$target" --locked
        ghostty_archive="$(find "target/$target/release/build" -path '*/ghostty_vt_sys-*/out/zig-out/lib/libghostty_vt.a' -print -quit)"
        if [[ -z "$ghostty_archive" ]]; then
            echo "error: built Ghostty archive not found for ISA audit" >&2
            exit 1
        fi
        ./scripts/check-portable-x86_64-isa.sh "$ghostty_archive"
        ./scripts/check-blake3-no-avx512-build.sh "target/$target/release/build"
    else
        cargo build -p cas --release --target "$target" --locked
    fi

    staging="$(mktemp -d)"
    cp "target/$target/release/cas" "$staging/"
    cp LICENSE "$staging/"
    if [[ "$target" == "x86_64-unknown-linux-gnu" ]]; then
        ./scripts/test-check-portable-x86_64-isa.sh "$staging/cas"
    fi
    tar -czvf "$DIST_DIR/cas-$target.tar.gz" -C "$staging" cas LICENSE
    rm -rf "$staging"
done

echo ""
echo "=== Local Audit Complete ==="
ls -lh "$DIST_DIR"/*.tar.gz

echo "Pushing annotated tag $TAG to trigger the authoritative Release workflow..."
git push origin "$TAG"

if "$MANUAL_PUBLISH"; then
    echo ""
    echo "WARNING: emergency manual publishing is intentionally competing with CI."
    echo "Use only after disabling/cancelling the workflow or when its runners cannot publish."
    gh release create "$TAG" \
        --repo "${RELEASE_REPO:-pippenz/cas}" \
        --title "CAS $TAG" \
        --generate-notes \
        "$DIST_DIR"/*.tar.gz
    echo "Manual release created. Run scripts/release-published-receipt.sh $TAG before announcing digests."
else
    echo "CI now owns release creation. Do not upload dist/local-audit archives or announce their digests."
    echo "After CI publishes both assets, run scripts/release-published-receipt.sh $TAG."
fi
