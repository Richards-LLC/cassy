#!/usr/bin/env bash
# Script to update the Homebrew formula with new version and SHA256 hashes
# Usage: ./update-formula.sh <version>
# Example: ./update-formula.sh 0.3.0

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FORMULA_PATH="$SCRIPT_DIR/cas.rb"
REPO="Richards-LLC/cassy"

if [[ $# -lt 1 ]]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 0.3.0"
    exit 1
fi

VERSION="$1"
echo "Updating formula to version $VERSION..."

# Download and compute SHA256 for each platform
echo "Downloading macOS ARM64 binary..."
MACOS_SHA=$(curl -fsSL "https://github.com/${REPO}/releases/download/v${VERSION}/cas-aarch64-apple-darwin.tar.gz" | shasum -a 256 | cut -d' ' -f1)
echo "  SHA256: $MACOS_SHA"

echo "Downloading Linux x86_64 binary..."
LINUX_SHA=$(curl -fsSL "https://github.com/${REPO}/releases/download/v${VERSION}/cas-x86_64-unknown-linux-gnu.tar.gz" | shasum -a 256 | cut -d' ' -f1)
echo "  SHA256: $LINUX_SHA"

# Update the formula
echo "Updating formula..."

# Update version
sed -i.bak "s/version \"[^\"]*\"/version \"${VERSION}\"/" "$FORMULA_PATH"

# Update SHA256 hashes
sed -i.bak "s/sha256 \"REPLACE_WITH_ACTUAL_SHA256_FOR_MACOS_ARM64\"/sha256 \"${MACOS_SHA}\"/" "$FORMULA_PATH"
sed -i.bak "s/sha256 \"REPLACE_WITH_ACTUAL_SHA256_FOR_LINUX_X86_64\"/sha256 \"${LINUX_SHA}\"/" "$FORMULA_PATH"

# Also update if there are existing hashes (for re-running)
sed -i.bak -E "s/(on_arm.*sha256 \")[^\"]*(\")/\1${MACOS_SHA}\2/" "$FORMULA_PATH"
sed -i.bak -E "s/(on_intel.*sha256 \")[^\"]*(\")/\1${LINUX_SHA}\2/" "$FORMULA_PATH"

# Clean up backup files
rm -f "$FORMULA_PATH.bak"

echo ""
echo "Formula updated successfully!"
echo ""
echo "Next steps:"
echo "1. Commit the updated formula"
echo "2. Push to the homebrew-cas repository"
echo "3. Users can update with: brew upgrade cas"
