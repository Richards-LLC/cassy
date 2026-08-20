#!/usr/bin/env bash
# Locate prebuilt release artifacts for an exact commit (cas-3b7c0).
#
# Prints `found=`, `run-id=` and `reason=` for the tag-time release workflow.
#
# FAIL-SAFE, NOT FAIL-CLOSED. Every failure mode — no prebuild run, a partial
# run, expired artifacts, an API outage — reports found=false, which routes the
# release back to the pre-existing cold build path. The prebuild is an
# accelerator; it can never be the reason a release cannot ship.
#
# The only accepted match is a *successful* Release Prebuild run whose
# head_sha is exactly the commit being published, so adopted bytes always come
# from the tagged tree.
set -euo pipefail

sha="${1:-${GITHUB_SHA:-}}"
repo="${GITHUB_REPOSITORY:-}"
gh_bin="${GH_BIN:-gh}"
workflow="${RELEASE_PREBUILD_WORKFLOW:-release-prebuild.yml}"
required_artifacts=("cas-x86_64-unknown-linux-gnu" "cas-aarch64-apple-darwin")

decline() {
    printf 'found=false\n'
    printf 'run-id=\n'
    printf 'reason=%s\n' "$1"
    exit 0
}

if [[ -z "$sha" ]]; then
    decline "no commit SHA to look up"
fi
if [[ -z "$repo" ]]; then
    decline "GITHUB_REPOSITORY is unset"
fi

runs_json=""
if ! runs_json="$("$gh_bin" api \
    "repos/$repo/actions/workflows/$workflow/runs?head_sha=$sha&status=success&per_page=20" 2>/dev/null)"; then
    decline "prebuild run lookup failed for $sha"
fi

mapfile -t run_ids < <(jq -r '.workflow_runs[]?.id // empty' <<<"$runs_json" 2>/dev/null || true)
if [[ "${#run_ids[@]}" -eq 0 ]]; then
    decline "no successful prebuild run for $sha"
fi

for run_id in "${run_ids[@]}"; do
    artifacts_json=""
    if ! artifacts_json="$("$gh_bin" api \
        "repos/$repo/actions/runs/$run_id/artifacts?per_page=100" 2>/dev/null)"; then
        continue
    fi
    complete=true
    for name in "${required_artifacts[@]}"; do
        live="$(jq -r --arg name "$name" \
            '[.artifacts[]? | select(.name == $name and .expired == false)] | length' \
            <<<"$artifacts_json" 2>/dev/null || echo 0)"
        if [[ "$live" != "1" ]]; then
            complete=false
            break
        fi
    done
    if "$complete"; then
        printf 'found=true\n'
        printf 'run-id=%s\n' "$run_id"
        printf 'reason=prebuild run %s carries every release asset for %s\n' "$run_id" "$sha"
        exit 0
    fi
done

decline "prebuild runs for $sha have no live copy of every release asset"
