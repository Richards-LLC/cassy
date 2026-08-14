#!/usr/bin/env bash
#
# Local release build script for CAS
#
# Builds release binaries for all targets from the local machine, packages them,
# and optionally creates a GitHub release. The Darwin artifact uses a native
# build and therefore requires a macOS host; the early host preflight below
# reports that requirement before any compiler work begins.
# Linux release builds deliberately recompile C dependencies from source, so
# they are slower than an incremental build but reproducibly enforce the
# portability policy that the post-build audits validate.
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

HOST_OS="$(uname -s)"
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

# Reject unsupported host/target combinations before bootstrapping toolchains
# or compiling, rather than failing deep in a native compiler invocation.
./scripts/check-release-host.sh "$HOST_OS" "${TARGETS[@]}"

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

ensure_release_tag() {
    if ! git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
        echo "Creating annotated tag $TAG on HEAD before release build..."
        git tag -a "$TAG" -m "$TAG"
    fi

    ./scripts/check-release-preflight.sh "$TAG"
}

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
# cargo-zigbuild resolves its Zig executable through PATH rather than $ZIG.
# Keep $ZIG for build scripts that consume it and expose the same pinned binary
# through PATH so cargo zigbuild is reproducible when run independently.
export PATH="$REPO_ROOT/.context/zig:$PATH"
echo "Zig: $(zig version)"

# A local release must reject lockfile/version/changelog/tag mistakes before
# starting the expensive artifact builds. Build-only remains a packaging aid,
# not a release action, so it deliberately does not create a tag. This runs
# after Zig is available because cargo check builds the native Ghostty layer.
if ! "$BUILD_ONLY"; then
    ensure_release_tag
fi

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
        # Cross-compile for Linux using zigbuild. Its target wrapper passes
        # Zig's portable x86_64 CPU baseline to C/C++ contributors; the audits
        # below remain the authoritative release portability proof.
        # C build-script output records compiler flags. A release portability
        # audit is meaningful only when its artifact reflects current source
        # and compiler policy, not C objects inherited from target/. Clear the
        # complete target so every C dependency recompiles with this release's
        # baseline policy (including BLAKE3's no-AVX-512 inputs). This costs an
        # incremental build, but prevents a cache-masked flag regression from
        # producing an apparently portable release.
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
        # Native build for macOS
        cargo build -p cas --release --target "$target" --locked
    fi

    echo "Packaging $target..."
    STAGING="$(mktemp -d)"
    cp "target/$target/release/cas" "$STAGING/"
    cp LICENSE "$STAGING/"
    if [[ "$target" == "x86_64-unknown-linux-gnu" ]]; then
        ./scripts/test-check-portable-x86_64-isa.sh "$STAGING/cas"
    fi
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
