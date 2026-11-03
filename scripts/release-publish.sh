#!/usr/bin/env bash
# Tag and publish one audited release from release.yml (cas-f95b).
#
# Usage: scripts/release-publish.sh --version <x.y.z> --sha <commit> --assets <dir>
#            [--tag <name>] [--prerelease] [--notes-out <file>]
#
# Runs in the release workflow after the packaged assets passed the ISA audit
# and the launch smoke. It:
#   1. refuses an existing release object for the tag (never replaces bytes
#      that a receipt may already have announced);
#   2. writes the notes: the CHANGELOG section for the version plus the
#      release timer, "PR #N opened -> published", with a marker that the
#      host's install-on-publish step completes with the install time;
#   3. creates the annotated tag at <commit> and pushes it (an existing remote
#      tag is accepted only when it already peels to <commit>);
#   4. creates the GitHub release with the assets.
#
# --prerelease publishes a dry run: GitHub's latest-release endpoint, which
# `cas update` and the installer read, never returns a prerelease.
#
# Environment: GITHUB_REPOSITORY (owner/repo), GH_BIN (default gh),
# RELEASE_PUBLISH_NOW (epoch override for tests).
set -euo pipefail

version=""
sha=""
assets=""
tag=""
prerelease=false
notes_out=""
while (($#)); do
    case "$1" in
        --version) version="${2:?}"; shift 2 ;;
        --sha) sha="${2:?}"; shift 2 ;;
        --assets) assets="${2:?}"; shift 2 ;;
        --tag) tag="${2:?}"; shift 2 ;;
        --prerelease) prerelease=true; shift ;;
        --notes-out) notes_out="${2:?}"; shift 2 ;;
        *)
            printf 'usage: %s --version <x.y.z> --sha <commit> --assets <dir> [--tag <name>] [--prerelease] [--notes-out <file>]\n' "$0" >&2
            exit 2
            ;;
    esac
done
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { printf 'error: --version must be x.y.z, got %s\n' "$version" >&2; exit 2; }
[[ -n "$sha" && -d "$assets" ]] || { printf 'error: --sha and an existing --assets directory are required\n' >&2; exit 2; }
tag="${tag:-v$version}"
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY must name owner/repo}"
gh_bin="${GH_BIN:-gh}"

shopt -s nullglob
archives=("$assets"/*.tar.gz)
shopt -u nullglob
for required in cas-x86_64-unknown-linux-gnu.tar.gz cas-aarch64-apple-darwin.tar.gz; do
    [[ -f "$assets/$required" ]] || { printf 'error: missing release asset %s in %s\n' "$required" "$assets" >&2; exit 1; }
done

# 1. A published release is immutable for announcement purposes.
if "$gh_bin" release view "$tag" --repo "$repo" >/dev/null 2>&1; then
    printf '::error::Release %s already exists; refusing to replace its assets. See docs/RELEASE_SLACK_RUBRIC.md#recovering-a-failed-or-partial-release before rerunning.\n' "$tag"
    exit 1
fi

# 2. Notes: the CHANGELOG section, then the timer.
notes="${notes_out:-$(mktemp)}"
awk -v version="$version" '
    $0 ~ "^## \\[" version "\\]" { in_section = 1; next }
    in_section && /^## \[/ { exit }
    in_section { print }
' CHANGELOG.md | sed -e '/./,$!d' >"$notes"
if [[ ! -s "$notes" ]]; then
    printf 'Release %s.\n' "$tag" >"$notes"
fi

now="${RELEASE_PUBLISH_NOW:-$(date -u +%s)}"
iso() { date -u -d "@$1" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -r "$1" +%Y-%m-%dT%H:%M:%SZ; }
pr_line=""
pr_json="$("$gh_bin" api "repos/$repo/commits/$sha/pulls" \
    --jq '[.[] | select(.merged_at != null)] | first | {number, created_at} // empty' 2>/dev/null || true)"
pr_number=""
pr_created=""
if [[ -n "$pr_json" ]]; then
    pr_number="$(jq -r '.number // empty' <<<"$pr_json" 2>/dev/null || true)"
    pr_created="$(jq -r '.created_at // empty' <<<"$pr_json" 2>/dev/null || true)"
fi
if [[ -n "$pr_number" && -n "$pr_created" ]]; then
    opened="$(date -u -d "$pr_created" +%s 2>/dev/null || date -u -j -f "%Y-%m-%dT%H:%M:%SZ" "$pr_created" +%s 2>/dev/null || true)"
fi
{
    printf '\n---\n\n'
    if [[ -n "${opened:-}" ]]; then
        minutes=$(( (now - opened + 59) / 60 ))
        printf '**Release timer:** PR #%s opened %s → published %s: %s min. Installed: pending.\n' \
            "$pr_number" "$(iso "$opened")" "$(iso "$now")" "$minutes"
        printf '<!-- cas-release-timer pr=%s opened=%s published=%s -->\n' "$pr_number" "$opened" "$now"
    else
        printf '**Release timer:** published %s; no merged PR found for %s. Installed: pending.\n' "$(iso "$now")" "$sha"
        printf '<!-- cas-release-timer pr= opened= published=%s -->\n' "$now"
    fi
} >>"$notes"

# 3. The annotated tag. A retry after a partial run may find it already pushed.
remote_tag="$(git ls-remote --tags origin "refs/tags/$tag^{}" | cut -f1)"
if [[ -z "$remote_tag" ]]; then
    remote_tag="$(git ls-remote --tags origin "refs/tags/$tag" | cut -f1)"
fi
if [[ -n "$remote_tag" ]]; then
    if [[ "$remote_tag" != "$sha" ]]; then
        printf '::error::Tag %s already exists at %s, not %s; refusing to publish.\n' "$tag" "$remote_tag" "$sha"
        exit 1
    fi
    printf 'tag %s already peels to %s\n' "$tag" "$sha"
else
    git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
        tag -a "$tag" -m "$tag" "$sha"
    git push origin "refs/tags/$tag"
    printf 'pushed annotated tag %s at %s\n' "$tag" "$sha"
fi

# 4. The release.
release_args=(release create "$tag" --repo "$repo" --verify-tag --title "CAS $tag" --notes-file "$notes")
if "$prerelease"; then
    release_args+=(--prerelease)
fi
"$gh_bin" "${release_args[@]}" "${archives[@]}"
printf 'published %s (%s) from %s\n' "$tag" "$([[ "$prerelease" == true ]] && echo prerelease || echo release)" "$sha"
