#!/usr/bin/env bash
# Standing CI policy: parsed workflow/ruleset contracts, execution, then mutations.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
python3 scripts/ci_tiers/policy.py check
bash scripts/ci_tiers/executable-contracts.sh
python3 scripts/ci_tiers/test-policy.py
bash scripts/test-ci-journey-evidence-upload.sh
