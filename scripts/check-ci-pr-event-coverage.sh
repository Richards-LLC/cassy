#!/usr/bin/env bash
# Decide whether a factory branch-push CI lane may stand down because a pull
# request event is already validating the identical head commit (cas-3e14).
#
# A factory push and a pull request can both produce a CI run for the same head
# SHA, and the branch-push copy is then pure duplication. It is also the last
# check to report, so a merge that waits for every check waits for it: measured
# on PR #479, the push-event Scoped Validation finished 9 seconds before the
# merge, 8 minutes after the required lanes were green.
#
# Standing down is only safe when the pull request event validates the SAME
# commit, so the only evidence accepted here is an OPEN pull request whose head
# SHA is exactly this commit. Then:
#   - a default-branch PR runs the required Fast Validation tier on that SHA,
#     which is strictly stronger than this lane; and
#   - an epic-targeted PR runs this same lane on that SHA.
# Everything else — no pull request, a pull request whose head has moved on, an
# unparseable answer, a missing CLI, or an API failure — leaves covered=false
# and the lane runs. A factory branch merged into an epic by `git merge --no-ff`
# never has a pull request, so it always validates here.
set -euo pipefail

output="${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
event="${GITHUB_EVENT_NAME:-}"
sha="${GITHUB_SHA:-}"
repository="${GITHUB_REPOSITORY:-}"

# Fail-closed default, written before any lookup can go wrong.
printf 'covered=false\n' >>"$output"

if [[ "$event" != "push" ]]; then
    echo "Pull request dedupe applies only to branch pushes; validating ${event:-unknown} normally."
    exit 0
fi

if [[ -z "$repository" || -z "$sha" ]]; then
    echo "::warning::Repository or head SHA unavailable; running the lane."
    exit 0
fi

if ! command -v gh >/dev/null; then
    echo "::warning::gh unavailable for the pull request lookup; running the lane."
    exit 0
fi

if ! pulls="$(gh api "repos/$repository/commits/$sha/pulls" \
    --jq "[.[] | select(.state == \"open\") | select(.head.sha == \"$sha\") | .number] | @tsv" \
    2>/dev/null)"; then
    echo "::warning::Could not determine whether a pull request covers $sha; running the lane."
    exit 0
fi

# `@tsv` on an empty array yields an empty line, so treat blank as "no cover".
read -r -a pr_numbers <<<"${pulls//$'\t'/ }"
for number in "${pr_numbers[@]:-}"; do
    [[ "$number" =~ ^[0-9]+$ ]] || continue
    printf 'covered=true\npr-number=%s\n' "$number" >>"$output"
    echo "Pull request #$number has head $sha; its pull request run validates this exact commit, so the duplicate push-event lane stands down."
    exit 0
done

echo "No open pull request has head $sha; running the lane."
