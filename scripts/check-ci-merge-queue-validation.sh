#!/usr/bin/env bash
# Main pushes may reuse a successful queue's exact-tree artifact. Queue entries
# may reuse a completed train full-set artifact when main is unchanged and the
# remaining source-to-queue delta is release plumbing. The shared consumer also
# checks the tree-bound cas/full-gate status; status-only or partial proof never
# skips the full graph.
set -euo pipefail

output="${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
event="${GITHUB_EVENT_NAME:-}"
ref="${GITHUB_REF:-}"
repository="${GITHUB_REPOSITORY:-}"

printf 'run-fast-validation=true\njourneys-reuse=false\n' >>"$output"

full_gate_reuse() {
    # Failure or unavailable evidence always leaves the full graph selected.
    python3 scripts/train-ci.py reuse --repo . --github-output "$output" \
        || echo "::warning::Full-set proof unavailable; running the complete shared gate."
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
