#!/usr/bin/env bash
# Decide whether a main-push heavy CI job may reuse protected-PR validation.
# Any missing or ambiguous evidence deliberately leaves run-heavy=true.
set -euo pipefail

output="${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
event="${GITHUB_EVENT_NAME:-}"
ref="${GITHUB_REF:-}"
repository="${GITHUB_REPOSITORY:-}"

printf 'run-heavy=true\n' >>"$output"

if [[ "$event" != "push" || "$ref" != "refs/heads/main" ]]; then
    echo "Tree dedupe applies only to main pushes; running heavy validation for ${event:-unknown} ${ref:-unknown}."
    exit 0
fi

tree_hash="$(git rev-parse 'HEAD^{tree}')"
printf 'tree-hash=%s\n' "$tree_hash" >>"$output"
marker="pr-validated-tree-$tree_hash"

if [[ -z "$repository" ]] || ! command -v gh >/dev/null || ! command -v jq >/dev/null; then
    echo "::warning::Tree validation lookup prerequisites unavailable; running heavy validation."
    exit 0
fi

if ! artifacts="$(gh api "/repos/$repository/actions/artifacts?name=$marker&per_page=100" 2>/dev/null)"; then
    echo "::warning::Could not query prior tree validation; running heavy validation."
    exit 0
fi

mapfile -t run_ids < <(jq -r '.artifacts[] | select(.expired == false) | .workflow_run.id // empty' <<<"$artifacts")
for run_id in "${run_ids[@]}"; do
    [[ "$run_id" =~ ^[0-9]+$ ]] || continue
    if ! run="$(gh api "/repos/$repository/actions/runs/$run_id" 2>/dev/null)"; then
        continue
    fi
    if jq -e '.event == "pull_request" and .status == "completed" and .conclusion == "success"' <<<"$run" >/dev/null; then
        run_url="$(jq -r '.html_url' <<<"$run")"
        [[ "$run_url" == https://* ]] || continue
        printf 'run-heavy=false\nprior-run-url=%s\n' "$run_url" >>"$output"
        echo "Tree $tree_hash already passed protected PR validation in $run_url; skipping duplicate main-push heavy work."
        exit 0
    fi
done

echo "No completed successful protected-PR receipt exists for tree $tree_hash; running heavy validation."
