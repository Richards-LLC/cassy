#!/usr/bin/env bash
#
# Local release build script for CAS
#
# Builds release binaries for all targets from the local machine,
# packages them, and optionally creates a GitHub release.
#
# Prerequisites:
#   - Rust toolchain (rustup)
#   - cargo-zigbuild: cargo install cargo-zigbuild
#   - gh CLI (for GitHub release creation)
#   - Environment variables: CAS_POSTHOG_API_KEY, CAS_SENTRY_DSN
#
# Usage:
#   ./scripts/release.sh              # Build + prompt for GitHub release
#   ./scripts/release.sh --build-only # Build without creating release
#   ./scripts/release.sh --publish    # Build + create release without prompting

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

TARGETS=(
    "aarch64-apple-darwin"
    "x86_64-unknown-linux-gnu"
)

DIST_DIR="$REPO_ROOT/dist"
BUILD_ONLY=false
AUTO_PUBLISH=false

for arg in "$@"; do
    case "$arg" in
        --build-only) BUILD_ONLY=true ;;
        --publish) AUTO_PUBLISH=true ;;
        -h|--help)
            echo "Usage: $0 [--build-only | --publish]"
            echo ""
            echo "  --build-only   Build tarballs only, skip GitHub release"
            echo "  --publish      Build and create GitHub release without prompting"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg"
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Load .env if present
# ---------------------------------------------------------------------------
if [ -f "$REPO_ROOT/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$REPO_ROOT/.env"
    set +a
fi

# ---------------------------------------------------------------------------
# Version & tag
# ---------------------------------------------------------------------------
VERSION="$(grep -m1 '^version = "' cas-cli/Cargo.toml | sed -E 's/^version = "([^"]+)"/\1/')"
TAG="v$VERSION"

echo "=== CAS Release Build ==="
echo "Version:  $VERSION"
echo "Tag:      $TAG"
echo "Targets:  ${TARGETS[*]}"
echo ""

# ---------------------------------------------------------------------------
# Pre-flight checks
# ---------------------------------------------------------------------------
if [ -z "${CAS_POSTHOG_API_KEY:-}" ]; then
    echo "error: CAS_POSTHOG_API_KEY is not set"
    exit 1
fi

if [ -z "${CAS_SENTRY_DSN:-}" ]; then
    echo "warning: CAS_SENTRY_DSN is not set — crash reporting will be disabled in this build"
fi

if ! command -v cargo-zigbuild &>/dev/null; then
    echo "error: cargo-zigbuild not found. Install with: cargo install cargo-zigbuild"
    exit 1
fi

if ! "$BUILD_ONLY" && ! command -v gh &>/dev/null; then
    echo "error: gh CLI not found. Install with: brew install gh"
    exit 1
fi

# A registered migration changes the doctor/status component snapshots, while
# the scoped release suites do not build that integration test.  Keep this
# before builds and, crucially, before create_release can create or push TAG.
./scripts/check-release-migration-snapshots.sh

# ---------------------------------------------------------------------------
# Bootstrap Zig if needed
# ---------------------------------------------------------------------------
if [ ! -x ".context/zig/zig" ]; then
    echo "Bootstrapping Zig..."
    ./scripts/bootstrap-zig.sh
fi
export ZIG="$REPO_ROOT/.context/zig/zig"
echo "Zig: $("$ZIG" version)"

# ---------------------------------------------------------------------------
# Ensure git submodules
# ---------------------------------------------------------------------------
git submodule update --init --recursive

# ---------------------------------------------------------------------------
# Ensure Rust targets are installed
# ---------------------------------------------------------------------------
INSTALLED_TARGETS="$(rustup target list --installed)"
for target in "${TARGETS[@]}"; do
    if ! echo "$INSTALLED_TARGETS" | grep -q "^${target}$"; then
        echo "Installing Rust target: $target"
        rustup target add "$target"
    fi
done

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

for target in "${TARGETS[@]}"; do
    echo ""
    echo "=== Building $target ==="

    if [[ "$target" == *"linux"* ]]; then
        # Cross-compile for Linux using zigbuild
        cargo zigbuild -p cas --release --target "$target"
        ./scripts/check-portable-x86_64-isa.sh "target/$target/release/cas"
    else
        # Native build for macOS
        cargo build -p cas --release --target "$target"
    fi

    echo "Packaging $target..."
    STAGING="$(mktemp -d)"
    cp "target/$target/release/cas" "$STAGING/"
    cp LICENSE "$STAGING/"
    tar -czvf "$DIST_DIR/cas-$target.tar.gz" -C "$STAGING" cas LICENSE
    rm -rf "$STAGING"

    echo "Built: dist/cas-$target.tar.gz"
done

echo ""
echo "=== Build Complete ==="
ls -lh "$DIST_DIR"/*.tar.gz

# ---------------------------------------------------------------------------
# GitHub release
# ---------------------------------------------------------------------------
if "$BUILD_ONLY"; then
    echo ""
    echo "Tarballs are in dist/. Skipping GitHub release (--build-only)."
    exit 0
fi

create_release() {
    # Ensure the tag exists locally
    if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
        echo "Tag $TAG does not exist. Creating it on HEAD..."
        git tag "$TAG"
    fi

    # Push the tag
    echo "Pushing tag $TAG..."
    git push origin "$TAG"

    # Generate release notes from commits since last tag
    PREV_TAG=$(git describe --tags --abbrev=0 "$TAG^" 2>/dev/null || echo "")
    if [ -n "$PREV_TAG" ]; then
        NOTES=$(git log --pretty=format:"- %s" "$PREV_TAG".."$TAG")
    else
        NOTES=$(git log --pretty=format:"- %s" -10)
    fi

    # Delete existing release if retag
    gh release delete "$TAG" --repo codingagentsystem/cas --yes 2>/dev/null || true

    # Create release on public repo
    gh release create "$TAG" \
        --repo codingagentsystem/cas \
        --title "CAS $TAG" \
        --notes "$NOTES" \
        "$DIST_DIR"/*.tar.gz

    echo ""
    echo "Release created: https://github.com/codingagentsystem/cas/releases/tag/$TAG"
}

if "$AUTO_PUBLISH"; then
    create_release
else
    echo ""
    read -p "Create GitHub release $TAG on codingagentsystem/cas? [y/N] " confirm
    if [[ "${confirm:-}" =~ ^[Yy]$ ]]; then
        create_release
    else
        echo "Skipped. Tarballs are in dist/."
    fi
fi
