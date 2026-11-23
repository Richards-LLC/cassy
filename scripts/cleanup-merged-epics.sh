#!/usr/bin/env bash
# Post-publication housekeeping: only this release's recorded epic branch.
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
# scripts/release.sh records the cut's branch and version in its commit.
# Read that immutable record, never infer train ownership from containment:
# an open next train is routinely synced to the exact published main tip.
version="$(git show "$released:cas-cli/Cargo.toml" | sed -n 's/^version = "\([^"]*\)"/\1/p')"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 1
shipped="$(
    while read -r cut; do
        [[ "$(git show -s --format=%s "$cut")" == "release: v$version" ]] || continue
        git show -s --format=%B "$cut" | sed -n \
            's|^Prepared by scripts/release.sh on \(epic/[^ ]*\) from [^ ]* [^ ]*\.$|\1|p'
    done < <(git log --format=%H --grep="^Release-Version: ${version//./[.]}$" "$released") \
        | sort -u
)"
if [[ -z "$shipped" ]]; then
    printf 'No recorded release epic for %s; no branches deleted\n' "$released"
    exit 0
fi
[[ "$shipped" != *$'\n'* ]] || { echo 'Ambiguous release epic; no branches deleted' >&2; exit 1; }
git check-ref-format "refs/heads/$shipped" >/dev/null
# The immutable cut identifies an enrolled train even if someone removed its
# authority ref. Require that exact authority rather than treating an empty
# namespace listing as permission to delete the train.
recorded_train="$(
    while read -r cut; do
        [[ "$(git show -s --format=%s "$cut")" == "release: v$version" ]] || continue
        git show -s --format=%B "$cut" | sed -n 's/^Release-Train: //p'
    done < <(git log --format=%H --grep="^Release-Version: ${version//./[.]}$" "$released") | sort -u
)"
train_args=()
if [[ -n "$recorded_train" ]]; then
    [[ "$recorded_train" =~ ^cas-[a-zA-Z0-9]+$ ]] || { echo 'Invalid or ambiguous recorded train; deletion held' >&2; exit 1; }
    train_args=(--train "$recorded_train")
fi
merged="$(git for-each-ref --merged="$released" --format='%(objectname) %(refname)' \
    "refs/remotes/origin/$shipped")"
while read -r sha ref; do
    [[ -n "$sha" ]] || continue
    [[ "$ref" == "refs/remotes/origin/$shipped" ]] || continue
    branch="refs/heads/${ref#refs/remotes/origin/}"
    # Enrollment survives merged/terminal tasks. An unreadable authority or
    # unsupported helper holds cleanup; train retirement is a separate action.
    enrolled="$("${CAS_BIN:-cas}" factory train publication is-enrolled --branch "$branch" "${train_args[@]}")"
    case "$enrolled" in
        true) printf 'Preserved enrolled train %s\n' "$branch"; continue ;;
        false) ;;
        *) printf 'Unknown train enrollment for %s; deletion held\n' "$branch" >&2; exit 1 ;;
    esac
    # A branch that advances after the fetch must survive. Never force away
    # new work or an epic merely merged into some later default-branch tip.
    git push --force-with-lease="$branch:$sha" origin ":$branch"
    printf 'Deleted merged epic %s at %s (release %s)\n' "$branch" "$sha" "$released"
done <<< "$merged"
printf 'Merged epic cleanup complete for release %s\n' "$released"
