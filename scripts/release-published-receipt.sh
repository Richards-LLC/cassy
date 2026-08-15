#!/usr/bin/env bash
# Print machine-readable digest fields only after the published GitHub release
# contains both CI assets and locally downloaded bytes match GitHub metadata.
set -euo pipefail

usage() {
    echo "Usage: scripts/release-published-receipt.sh <vX.Y.Z> [--write-draft <path>]" >&2
}

if [[ "$#" -ne 1 && "$#" -ne 3 ]]; then
    usage
    exit 2
fi

tag="$1"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: expected an annotated release tag like vX.Y.Z; got $tag" >&2
    exit 2
fi

draft=""
if [[ "$#" -eq 3 ]]; then
    if [[ "$2" != "--write-draft" || ! -f "$3" ]]; then
        echo "error: --write-draft requires an existing draft path" >&2
        exit 2
    fi
    draft="$3"
fi

gh_bin="${GH_BIN:-gh}"
repo="${RELEASE_REPO:-pippenz/cas}"
linux_asset="cas-x86_64-unknown-linux-gnu.tar.gz"
macos_asset="cas-aarch64-apple-darwin.tar.gz"
workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

release_json="$workdir/release.json"
if ! "$gh_bin" release view "$tag" --repo "$repo" --json isDraft,publishedAt,assets >"$release_json"; then
    echo "error: published release $tag is not available yet" >&2
    exit 1
fi

if [[ "$(jq -r 'if has("isDraft") then .isDraft else true end' "$release_json")" != "false" ]] \
    || [[ -z "$(jq -r '.publishedAt // empty' "$release_json")" ]]; then
    echo "error: release $tag exists but is not published yet" >&2
    exit 1
fi

digest_for() {
    jq -r --arg name "$1" '.assets[]? | select(.name == $name) | .digest // empty' "$release_json"
}

for asset in "$linux_asset" "$macos_asset"; do
    digest="$(digest_for "$asset")"
    if [[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
        echo "error: release $tag is partial; required asset $asset with a SHA-256 digest is not published yet" >&2
        exit 1
    fi
done

for asset in "$linux_asset" "$macos_asset"; do
    "$gh_bin" release download "$tag" --repo "$repo" --dir "$workdir" --pattern "$asset"
    local_digest="$(sha256sum "$workdir/$asset" | awk '{print $1}')"
    github_digest="$(digest_for "$asset" | sed 's/^sha256://')"
    if [[ "$local_digest" != "$github_digest" ]]; then
        echo "error: downloaded $asset digest $local_digest does not match GitHub digest $github_digest" >&2
        exit 1
    fi
done

linux_sha="$(sha256sum "$workdir/$linux_asset" | awk '{print $1}')"
macos_sha="$(sha256sum "$workdir/$macos_asset" | awk '{print $1}')"

replace_draft_token() {
    local token="$1" value="$2" replacement
    replacement="$(mktemp)"
    if ! awk -v token="$token" -v value="$value" '
        BEGIN { replacements = 0 }
        {
            occurrences = gsub(token, value)
            replacements += occurrences
            print
        }
        END { exit replacements >= 1 ? 0 : 1 }
    ' "$draft" >"$replacement"; then
        rm -f "$replacement"
        echo "error: draft $draft must contain a $token placeholder" >&2
        exit 1
    fi
    mv "$replacement" "$draft"
}

if [[ -n "$draft" ]]; then
    replace_draft_token '{{LINUX_SHA256}}' "$linux_sha"
    replace_draft_token '{{MACOS_SHA256}}' "$macos_sha"
fi

printf 'TAG=%s\n' "$tag"
printf 'PUBLISHED_AT=%s\n' "$(jq -r '.publishedAt' "$release_json")"
printf 'LINUX_ASSET=%s\n' "$linux_asset"
printf 'LINUX_SHA256=%s\n' "$linux_sha"
printf 'MACOS_ASSET=%s\n' "$macos_asset"
printf 'MACOS_SHA256=%s\n' "$macos_sha"
