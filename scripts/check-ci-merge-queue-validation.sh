#!/usr/bin/env bash
# Decide whether Fast Validation can reuse a prior proof of the exact same Git
# tree: a main push reuses a successful merge-queue run, and a merge-queue entry
# reuses the release train's full-gate receipt (cas-4cb8). Missing or ambiguous
# evidence deliberately keeps the full suite enabled.
set -euo pipefail

output="${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
event="${GITHUB_EVENT_NAME:-}"
ref="${GITHUB_REF:-}"
repository="${GITHUB_REPOSITORY:-}"

printf 'run-fast-validation=true\n' >>"$output"

# cas-4cb8: a merge-queue entry reuses the release train's full-gate receipt.
# The train posts a `cas/full-gate` success status on the release PR head it
# proved, naming that head's tree. The queue tree must be the PR head's tree
# (the PR was up to date with main), and the latest status for that context
# must be a PASS for exactly this tree. Any missing, failed, superseded or
# ambiguous evidence runs the full Fast Validation lanes.
full_gate_reuse() {
    local tree_hash pr_number pr head head_tree statuses latest url
    tree_hash="$(git rev-parse 'HEAD^{tree}')"
    printf 'tree-hash=%s\n' "$tree_hash" >>"$output"
    if [[ ! "$ref" =~ ^refs/heads/gh-readonly-queue/[^/]+/pr-([0-9]+)-[0-9a-f]{40}$ ]]; then
        echo "Merge-queue ref $ref names no pull request; running Fast Validation."
        return 0
    fi
    pr_number="${BASH_REMATCH[1]}"
    if [[ -z "$repository" ]] || ! command -v gh >/dev/null || ! command -v jq >/dev/null; then
        echo "::warning::Full-gate receipt lookup prerequisites unavailable; running Fast Validation."
        return 0
    fi
    if ! pr="$(gh api "/repos/$repository/pulls/$pr_number" 2>/dev/null)" \
        || ! head="$(jq -er '.head.sha | select(test("^[0-9a-f]{40}$"))' <<<"$pr" 2>/dev/null)"; then
        echo "::warning::Could not read pull request $pr_number; running Fast Validation."
        return 0
    fi
    if ! head_tree="$(gh api "/repos/$repository/git/commits/$head" 2>/dev/null | jq -er '.tree.sha' 2>/dev/null)" \
        || [[ "$head_tree" != "$tree_hash" ]]; then
        echo "PR #$pr_number head $head is not the queue tree $tree_hash (tree ${head_tree:-unknown}); running Fast Validation."
        return 0
    fi
    if ! statuses="$(gh api "/repos/$repository/commits/$head/statuses?per_page=100" 2>/dev/null)"; then
        echo "::warning::Could not read the statuses of $head; running Fast Validation."
        return 0
    fi
    # The statuses API lists newest first; only the latest cas/full-gate counts.
    latest="$(jq -c '[.[] | select(.context == "cas/full-gate")] | first // empty' <<<"$statuses" 2>/dev/null || true)"
    if [[ -z "$latest" ]] \
        || ! jq -e --arg tree "$tree_hash" \
            '.state == "success" and .description == ("PASS tree=" + $tree)' <<<"$latest" >/dev/null 2>&1; then
        echo "No current full-gate PASS receipt for tree $tree_hash on $head; running Fast Validation."
        return 0
    fi
    url="$(jq -r '.target_url // empty' <<<"$latest")"
    [[ "$url" == https://* ]] || url="https://github.com/$repository/commit/$head"
    printf 'run-fast-validation=false\nreuse-source=full-gate\nvalidating-run-id=full-gate:%s\nprior-run-url=%s\n' \
        "$head" "$url" >>"$output"
    echo "::notice title=Fast Validation reused the full gate::Tree $tree_hash is PR #$pr_number head $head, which the release train's full gate proved ($url); skipping the duplicate queue preflight, suite and doctests. macOS Check still runs."
}

if [[ "$event" == "merge_group" ]]; then
    full_gate_reuse
    exit 0
fi

if [[ "$event" != "push" || "$ref" != "refs/heads/main" ]]; then
    echo "Merge-queue tree dedupe applies only to main pushes; running Fast Validation for ${event:-unknown} ${ref:-unknown}."
    exit 0
fi

tree_hash="$(git rev-parse 'HEAD^{tree}')"
printf 'tree-hash=%s\n' "$tree_hash" >>"$output"
marker="merge-queue-validated-tree-$tree_hash"

if [[ -z "$repository" ]] || ! command -v gh >/dev/null || ! command -v jq >/dev/null; then
    echo "::warning::Merge-queue validation lookup prerequisites unavailable; running Fast Validation."
    exit 0
fi

if ! artifacts="$(gh api "/repos/$repository/actions/artifacts?name=$marker&per_page=100" 2>/dev/null)"; then
    echo "::warning::Could not query merge-queue validation receipts; running Fast Validation."
    exit 0
fi

mapfile -t run_ids < <(jq -r '.artifacts[] | select(.expired == false) | .workflow_run.id // empty' <<<"$artifacts")
for run_id in "${run_ids[@]}"; do
    [[ "$run_id" =~ ^[0-9]+$ ]] || continue
    if ! run="$(gh api "/repos/$repository/actions/runs/$run_id" 2>/dev/null)"; then
        continue
    fi
    if jq -e '.event == "merge_group" and .status == "completed" and .conclusion == "success"' <<<"$run" >/dev/null; then
        run_url="$(jq -r '.html_url' <<<"$run")"
        [[ "$run_url" == https://* ]] || continue
        printf 'run-fast-validation=false\nreuse-source=merge-queue\nvalidating-run-id=%s\nprior-run-url=%s\n' "$run_id" "$run_url" >>"$output"
        echo "::notice title=Fast Validation deduplicated::Tree $tree_hash already passed successful merge-queue run $run_id ($run_url); skipping duplicate main-push Fast Validation and macOS work."
        exit 0
    fi
done

echo "No completed successful merge-queue receipt exists for tree $tree_hash; running Fast Validation."
