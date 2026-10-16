#!/usr/bin/env bash
# Resolve the event delta once. First factory pushes use shared epic ancestry,
# never a local branch, commit subject, or caller-provided HEAD guess.
set -euo pipefail
[[ $# == 2 ]] || { echo 'usage: resolve-ci-base.sh <event-base> <default-ref>' >&2; exit 2; }
base="$1"
default_ref="$2"
if [[ -z "$base" || "$base" =~ ^0+$ ]]; then
    [[ "$base" =~ ^0+$ && "${GITHUB_EVENT_NAME:-}" == push \
        && "${GITHUB_REF:-}" == refs/heads/* ]] || exit 1
    [[ "$default_ref" == origin/* && "$default_ref" != origin/factory/* \
        && "$default_ref" != origin/epic/* ]] || exit 1
    git check-ref-format "refs/remotes/$default_ref" >/dev/null || exit 1
    git show-ref --verify --quiet "refs/remotes/$default_ref" || exit 1
    base="$(git rev-parse --verify "$default_ref^{commit}")"
    if [[ "${GITHUB_REF:-}" == refs/heads/factory/* ]]; then
        # Use the shared fork, since a sibling may advance an epic while the
        # worker runs. Only fetched same-repository epic refs supply candidates.
        # Two incomparable maximal forks give no task parent and fail closed.
        parents=()
        while read -r candidate; do
            [[ -n "$candidate" ]] || continue
            if ! fork="$(git merge-base "$candidate" HEAD)"; then
                echo 'CI base: shared epic ancestry unavailable; refusing task scope' >&2
                exit 1
            fi
            parents+=("$fork")
        done < <(git for-each-ref --format='%(refname)' refs/remotes/origin/epic/)
        selected=''
        for candidate in "${parents[@]}"; do
            maximal=true
            for other in "${parents[@]}"; do
                if [[ "$candidate" != "$other" ]] && git merge-base --is-ancestor "$candidate" "$other"; then
                    maximal=false
                    break
                fi
            done
            if "$maximal"; then
                if [[ -n "$selected" && "$selected" != "$candidate" ]]; then
                    echo 'CI base: ambiguous shared epic parents; refusing task scope' >&2
                    exit 1
                fi
                selected="$candidate"
            fi
        done
        if [[ -n "$selected" ]] && ! git merge-base --is-ancestor "$selected" "$base"; then
            base="$selected"
        fi
    fi
fi
# Nonzero event bases preserve repair-push scope. Unknown refs fail closed.
git merge-base "$base" HEAD
