#!/usr/bin/env bash
# Release gate: a release that changes hub-web must carry a journey evaluation
# of exactly the bundle it ships (docs/qa/journey-evaluation.md). The key is
# the git tree of hub-web/dist: the bytes cas embeds, rebuilt by CI whenever
# the hub-web source changes.
#
# Usage: check-journey-evaluation.sh <worktree>
#
# Passes when hub-web/dist is absent, when it is unchanged since the last
# release tag (CAS_JOURNEY_BASE_REF overrides the tag), or when a committed
# docs/qa/journey-evaluations/*.md report names the current dist tree, has
# `blocking_findings: 0`, and records every catalog journey as PASS.
# Otherwise prints `BLOCKER journey-evaluation: ...` and exits 1.
set -euo pipefail

worktree="${1:?usage: check-journey-evaluation.sh <worktree>}"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
reports_dir="docs/qa/journey-evaluations"

block() {
    printf 'BLOCKER journey-evaluation: %s\n' "$1" >&2
    printf '  → run scripts/journey-eval.sh on the release candidate, have a taste-lane evaluator score it, and commit the report to %s (see docs/qa/journey-evaluation.md)\n' "$reports_dir" >&2
    exit 1
}

git -C "$worktree" rev-parse --verify -q 'HEAD:hub-web/dist' >/dev/null || {
    printf 'journey-evaluation: no hub-web/dist in HEAD; not applicable\n'
    exit 0
}

base="${CAS_JOURNEY_BASE_REF:-}"
if [[ -z "$base" ]]; then
    base="$(git -C "$worktree" describe --tags --abbrev=0 --match 'v[0-9]*' HEAD 2>/dev/null || true)"
fi
if [[ -n "$base" ]] && git -C "$worktree" diff --quiet "$base" HEAD -- hub-web/dist; then
    printf 'journey-evaluation: hub-web/dist unchanged since %s; not required\n' "$base"
    exit 0
fi

tree="$(git -C "$worktree" rev-parse 'HEAD:hub-web/dist')"
mapfile -t reports < <(git -C "$worktree" grep -l -E "^[-* ]*hub_web_dist: *$tree\b" HEAD -- "$reports_dir" 2>/dev/null \
    | sed 's#^HEAD:##' | grep -v '/TEMPLATE\.md$' || true)
[[ ${#reports[@]} -gt 0 ]] || block "hub-web/dist changed since ${base:-<no release tag>} and no committed report in $reports_dir names hub_web_dist $tree"

ids_json="$(CAS_JOURNEYS_ROOT="$worktree" python3 "$script_dir/journeys-for-diff.py" --all)" \
    || block "cannot read the journey catalog in $worktree"
mapfile -t ids < <(python3 -c 'import json,sys; [print(j["id"]) for j in json.load(sys.stdin)["journeys"]]' <<<"$ids_json")
[[ ${#ids[@]} -gt 0 ]] || block "the journey catalog lists no journeys"

failures=()
for report in "${reports[@]}"; do
    body="$(git -C "$worktree" show "HEAD:$report")"
    problems=()
    grep -Eq '^[-* ]*blocking_findings: *0 *$' <<<"$body" || problems+=("blocking_findings is not 0")
    for id in "${ids[@]}"; do
        grep -Eq "^\| *$id *\| *PASS *\|" <<<"$body" || problems+=("$id has no PASS row")
    done
    if [[ ${#problems[@]} -eq 0 ]]; then
        printf 'journey-evaluation: %s covers hub-web/dist tree %s (%d journeys)\n' "$report" "${tree:0:8}" "${#ids[@]}"
        exit 0
    fi
    failures+=("$report: $(IFS=';'; printf '%s' "${problems[*]}")")
done
block "no passing report for hub-web/dist tree ${tree:0:8}: ${failures[*]}"
