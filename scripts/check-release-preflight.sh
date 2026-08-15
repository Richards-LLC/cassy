#!/usr/bin/env bash
# Fail cheap release-input mistakes before any fat-LTO artifact build starts.
set -euo pipefail

usage() {
  echo "Usage: $0 [--local] <annotated-tag>" >&2
  echo "Example: $0 v2.57.0            # CI lane: re-fetches the remote tag object" >&2
  echo "Example: $0 --local v2.57.0    # pre-push lane: tag exists only locally" >&2
}

# The remote tag re-fetch below is inherently CI-side: it can only pass once the
# tag has been pushed. --local runs every other gate so the same checks can fail
# a bad release *before* the tag is pushed.
local_mode=false
tag=""
tag_seen=false
for arg in "$@"; do
  case "$arg" in
    --local|--no-remote-tag) local_mode=true ;;
    -h|--help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option $arg" >&2
      usage
      exit 2
      ;;
    *)
      if "$tag_seen"; then
        usage
        exit 2
      fi
      tag="$arg"
      tag_seen=true
      ;;
  esac
done

if ! "$tag_seen"; then
  usage
  exit 2
fi

if ! [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  echo "error: release tag must be v<semver>; got $tag" >&2
  exit 1
fi
version="${BASH_REMATCH[1]}"

if [ -n "$(git status --porcelain)" ]; then
  echo "error: release input is dirty; commit or discard every change before tagging" >&2
  exit 1
fi
echo "ok: working tree is clean"

# actions/checkout can materialize the event tag as its peeled commit/tree
# rather than retaining the tag object. Re-fetch the exact remote tag object
# before inspecting it, so this remains an annotated-tag check instead of an
# accidental check of checkout's local ref shape. In --local mode the tag has
# not been pushed yet, so the local ref is authoritative and the fetch would
# always fail with "couldn't find remote ref".
if "$local_mode"; then
  echo "ok: local mode; inspecting the local $tag object without a remote re-fetch"
else
  git fetch origin --no-tags --force "+refs/tags/$tag:refs/tags/$tag"
fi
if [ "$(git cat-file -t "refs/tags/$tag" 2>/dev/null || true)" != "tag" ]; then
  echo "error: $tag must exist locally as an annotated tag" >&2
  exit 1
fi
# `^{}` peels the verified tag object to the commit Actions checked out,
# rejecting a stale or re-pointed tag.
tag_commit="$(git rev-parse "refs/tags/$tag^{}")"
build_commit="$(git rev-parse HEAD)"
if [ "$tag_commit" != "$build_commit" ]; then
  echo "error: $tag peels to $tag_commit, but this build checks out $build_commit" >&2
  exit 1
fi
echo "ok: annotated tag $tag peels to the build commit"

release_crates=(
  "cas-cli/Cargo.toml"
  "crates/cas-types/Cargo.toml"
  "crates/cas-search/Cargo.toml"
  "crates/cas-store/Cargo.toml"
  "crates/cas-core/Cargo.toml"
  "crates/cas-mcp/Cargo.toml"
)
for manifest in "${release_crates[@]}"; do
  actual="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$manifest" | head -n 1)"
  if [ "$actual" != "$version" ]; then
    echo "error: $manifest is $actual; expected $version from $tag" >&2
    exit 1
  fi
done
echo "ok: all release-train crate versions match $tag"

if ! grep -Eq "^## \[$version\]( - [0-9]{4}-[0-9]{2}-[0-9]{2})?$" CHANGELOG.md; then
  echo "error: CHANGELOG.md is missing a heading for $version" >&2
  exit 1
fi
echo "ok: CHANGELOG.md contains $version"

"${CARGO:-cargo}" check --locked
echo "ok: cargo check --locked"
