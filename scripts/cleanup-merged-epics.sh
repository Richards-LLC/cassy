#!/usr/bin/env bash
# Post-publication housekeeping: only remote epic tips included in this release.
set -euo pipefail
[[ $# == 2 ]] || { echo 'usage: cleanup-merged-epics.sh <released-sha> <default-ref>' >&2; exit 2; }
released="$(git rev-parse --verify "$1^{commit}")"
default_ref="$2"
[[ "$default_ref" == origin/* && "$default_ref" != origin/epic/* \
    && "$default_ref" != origin/factory/* ]] || exit 1
git check-ref-format "refs/remotes/$default_ref" >/dev/null
default_branch="${default_ref#origin/}"
# The release checkout has full history. Refresh snapshots and prune refs from
# previous invocations so repeated cleanup is a no-op, including deleted epics.
git fetch --no-tags --prune origin \
    "+refs/heads/$default_branch:refs/remotes/$default_ref" \
    '+refs/heads/epic/*:refs/remotes/origin/epic/*'
git merge-base --is-ancestor "$released" "$default_ref"
merged="$(git for-each-ref --merged="$released" --format='%(objectname) %(refname)' \
    refs/remotes/origin/epic/)"
while read -r sha ref; do
    [[ -n "$sha" ]] || continue
    branch="refs/heads/${ref#refs/remotes/origin/}"
    # A branch that advances after the fetch must survive. Never force away
    # new work or an epic merely merged into some later default-branch tip.
    git push --force-with-lease="$branch:$sha" origin ":$branch"
    printf 'Deleted merged epic %s at %s (release %s)\n' "$branch" "$sha" "$released"
done <<< "$merged"
printf 'Merged epic cleanup complete for release %s\n' "$released"
